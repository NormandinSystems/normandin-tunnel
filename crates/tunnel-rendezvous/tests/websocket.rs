//! The rendezvous server over real WebSockets.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tunnel_proto::rendezvous::{ClientMessage, ErrorCode, ServerMessage};
use tunnel_rendezvous::{Config, spawn_local};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: std::net::SocketAddr) -> Socket {
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1"))
        .await
        .unwrap();
    socket
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    socket
        .send(Message::text(serde_json::to_string(message).unwrap()))
        .await
        .unwrap();
}

async fn recv(socket: &mut Socket) -> Option<ServerMessage> {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("timed out");
        match frame {
            Some(Ok(Message::Text(text))) => {
                return Some(serde_json::from_str(text.as_str()).unwrap());
            }
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
            Some(Ok(_)) => continue,
        }
    }
}

async fn create(socket: &mut Socket) -> u16 {
    send(socket, &ClientMessage::Create).await;
    match recv(socket).await {
        Some(ServerMessage::Created {
            nameplate,
            expires_in,
        }) => {
            assert!(expires_in > 0);
            nameplate
        }
        other => panic!("expected created, got {other:?}"),
    }
}

#[tokio::test]
async fn two_devices_pair_and_a_third_is_refused() {
    let (addr, server, _task) = spawn_local(Config::default()).await.unwrap();
    let mut a = connect(addr).await;
    let nameplate = create(&mut a).await;
    send(
        &mut a,
        &ClientMessage::Post {
            body: "QUFB".into(),
        },
    )
    .await;

    let mut b = connect(addr).await;
    send(&mut b, &ClientMessage::Join { nameplate }).await;
    assert_eq!(recv(&mut b).await, Some(ServerMessage::Joined));
    assert_eq!(
        recv(&mut b).await,
        Some(ServerMessage::Message {
            body: "QUFB".into()
        })
    );
    assert_eq!(recv(&mut a).await, Some(ServerMessage::PeerJoined));

    let mut c = connect(addr).await;
    send(&mut c, &ClientMessage::Join { nameplate }).await;
    assert_eq!(
        recv(&mut c).await,
        Some(ServerMessage::Error {
            code: ErrorCode::CodeInUse
        })
    );
    assert_eq!(
        recv(&mut c).await,
        None,
        "the connection closes after an error"
    );

    send(
        &mut b,
        &ClientMessage::Post {
            body: "QkJC".into(),
        },
    )
    .await;
    assert_eq!(
        recv(&mut a).await,
        Some(ServerMessage::Message {
            body: "QkJC".into()
        })
    );

    send(&mut a, &ClientMessage::Close).await;
    assert_eq!(recv(&mut b).await, Some(ServerMessage::PeerLeft));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.stats().rooms, 0);
}

#[tokio::test]
async fn unknown_and_malformed_requests_get_coded_errors() {
    let (addr, _server, _task) = spawn_local(Config::default()).await.unwrap();

    let mut s = connect(addr).await;
    send(&mut s, &ClientMessage::Join { nameplate: 4242 }).await;
    assert_eq!(
        recv(&mut s).await,
        Some(ServerMessage::Error {
            code: ErrorCode::UnknownCode
        })
    );

    let mut s = connect(addr).await;
    s.send(Message::text("{not json")).await.unwrap();
    assert_eq!(
        recv(&mut s).await,
        Some(ServerMessage::Error {
            code: ErrorCode::BadRequest
        })
    );

    let mut s = connect(addr).await;
    send(
        &mut s,
        &ClientMessage::Post {
            body: "QUFB".into(),
        },
    )
    .await;
    assert_eq!(
        recv(&mut s).await,
        Some(ServerMessage::Error {
            code: ErrorCode::BadRequest
        })
    );

    let mut s = connect(addr).await;
    s.send(Message::binary(vec![1u8, 2, 3])).await.unwrap();
    assert_eq!(
        recv(&mut s).await,
        Some(ServerMessage::Error {
            code: ErrorCode::BadRequest
        })
    );

    let mut s = connect(addr).await;
    create(&mut s).await;
    send(&mut s, &ClientMessage::Create).await;
    assert_eq!(
        recv(&mut s).await,
        Some(ServerMessage::Error {
            code: ErrorCode::BadRequest
        })
    );
}

#[tokio::test]
async fn oversized_messages_are_refused() {
    let (addr, _server, _task) = spawn_local(Config::default()).await.unwrap();
    let mut s = connect(addr).await;
    create(&mut s).await;
    let body = "A".repeat(tunnel_proto::rendezvous::MAX_BODY_BASE64_CHARS + 4);
    send(&mut s, &ClientMessage::Post { body }).await;
    assert_eq!(
        recv(&mut s).await,
        Some(ServerMessage::Error {
            code: ErrorCode::TooLarge
        })
    );

    // Past the WebSocket message limit the connection is simply dropped.
    let mut s = connect(addr).await;
    let _ = s.send(Message::text("x".repeat(64 * 1024))).await;
    assert_eq!(recv(&mut s).await, None);
}

#[tokio::test]
async fn rooms_expire() {
    let config = Config {
        room_lifetime: Duration::from_secs(1),
        ..Config::default()
    };
    let (addr, _server, _task) = spawn_local(config).await.unwrap();
    let mut a = connect(addr).await;
    let nameplate = create(&mut a).await;
    assert_eq!(
        recv(&mut a).await,
        Some(ServerMessage::Error {
            code: ErrorCode::Expired
        })
    );
    let mut b = connect(addr).await;
    send(&mut b, &ClientMessage::Join { nameplate }).await;
    assert_eq!(
        recv(&mut b).await,
        Some(ServerMessage::Error {
            code: ErrorCode::Expired
        })
    );
}

#[tokio::test]
async fn disconnecting_closes_the_room() {
    let (addr, server, _task) = spawn_local(Config::default()).await.unwrap();
    let mut a = connect(addr).await;
    let nameplate = create(&mut a).await;
    let mut b = connect(addr).await;
    send(&mut b, &ClientMessage::Join { nameplate }).await;
    assert_eq!(recv(&mut b).await, Some(ServerMessage::Joined));
    drop(a);
    assert_eq!(recv(&mut b).await, Some(ServerMessage::PeerLeft));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.stats().rooms, 0);
}

#[tokio::test]
async fn a_thousand_rooms_at_once() {
    let config = Config {
        creates_per_minute: 5000,
        connections_per_ip: 5000,
        ..Config::default()
    };
    let (addr, server, _task) = spawn_local(config).await.unwrap();
    let mut sockets = Vec::new();
    let mut nameplates = std::collections::HashSet::new();
    for _ in 0..1000 {
        let mut s = connect(addr).await;
        assert!(
            nameplates.insert(create(&mut s).await),
            "nameplates must be unique"
        );
        sockets.push(s);
    }
    assert_eq!(server.stats().rooms, 1000);
    assert!(
        nameplates.iter().any(|n| *n >= 1000),
        "moves to four digits when low numbers run short"
    );
}

#[tokio::test]
async fn health_check_answers() {
    let (addr, _server, _task) = spawn_local(Config::default()).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
}
