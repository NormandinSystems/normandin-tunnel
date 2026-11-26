//! Pairing without a rendezvous server.
//!
//! Two real engines and a relay on loopback. Neither engine has a pairing
//! server configured: the one showing the code waits at the meeting point
//! derived from the code's number, and the other dials it through the relay.

use tunnel_core::{SessionState, TransferState};
use tunnel_tests::*;

fn wrong_words(code: &str) -> String {
    let nameplate = code.split('-').next().unwrap();
    let wrong = format!("{nameplate}-aardvark-abandoned-abbreviate");
    assert_ne!(wrong, code);
    wrong
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_devices_pair_and_send_a_file_with_no_pairing_server() {
    let relay = relay().await;
    let (mut a, mut b) = (
        serverless_node(&relay, "Alpha"),
        serverless_node(&relay, "Bravo"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    // A device picks a four-digit number for itself.
    let nameplate: u16 = code.split('-').next().unwrap().parse().unwrap();
    assert!((1000..=9999).contains(&nameplate), "{code}");

    b.tunnel.join_code(&code, false).await.unwrap();
    let (safety_a, safety_b) = tokio::join!(a.wait_safety(), b.wait_safety());
    assert_eq!(safety_a, safety_b);
    a.tunnel.confirm_safety(true).await.unwrap();
    b.tunnel.confirm_safety(true).await.unwrap();
    tokio::join!(a.wait_active(), b.wait_active());

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("notes.bin");
    let body = pattern(11, 3 * 1024 * 1024 + 17);
    std::fs::write(&file, &body).unwrap();
    let (sent, received) = transfer(&mut a, &mut b, vec![file]).await;
    assert_eq!(sent.state, TransferState::Done);
    assert_eq!(received.state, TransferState::Done);
    assert_eq!(std::fs::read(b.saved("notes.bin")).unwrap(), body);

    // Text goes the other way too.
    b.tunnel.send_text("made it".to_string()).await.unwrap();
    assert_eq!(a.wait_text().await, "made it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_meeting_point_is_found_whichever_relay_it_is_on() {
    // The device typing the code knows only the number. It does not know
    // which relay the other device chose, so it tries every relay it has.
    let (one, two) = (relay().await, relay().await);
    for host_relay in [&one, &two] {
        let mut a = serverless_node_on(&[host_relay], "Alpha");
        let mut b = serverless_node_on(&[&one, &two], "Bravo");
        pair(&mut a, &mut b).await;
        a.tunnel.send_text("found you".to_string()).await.unwrap();
        assert_eq!(b.wait_text().await, "found you");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_code_is_different() {
    let relay = relay().await;
    let mut a = serverless_node(&relay, "Alpha");
    let mut seen = std::collections::HashSet::new();
    for _ in 0..4 {
        a.tunnel.create_code().await.unwrap();
        assert!(seen.insert(a.wait_code().await), "a code was shown twice");
        a.tunnel.end_session().await;
        a.wait_ended().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_words_fail_on_both_sides_and_use_up_the_code() {
    let relay = relay().await;
    let (mut a, mut b, mut c) = (
        serverless_node(&relay, "Alpha"),
        serverless_node(&relay, "Bravo"),
        serverless_node(&relay, "Charlie"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;

    // The right number finds the device; the wrong words get one try.
    b.tunnel
        .join_code(&wrong_words(&code), false)
        .await
        .unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_failed"));
    assert_eq!(a.wait_ended().await.as_deref(), Some("code_failed"));

    // The code is dead: the real one finds nobody now.
    c.tunnel.join_code(&code, false).await.unwrap();
    assert_eq!(c.wait_ended().await.as_deref(), Some("code_not_found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_code_nobody_is_showing_is_reported_as_not_found() {
    let relay = relay().await;
    let mut b = serverless_node(&relay, "Bravo");
    b.tunnel
        .join_code("4821-aardvark-abandoned-abbreviate", false)
        .await
        .unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_not_found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_device_is_told_the_code_is_in_use() {
    let relay = relay().await;
    let (mut a, mut b, mut c) = (
        serverless_node(&relay, "Alpha"),
        serverless_node(&relay, "Bravo"),
        serverless_node(&relay, "Charlie"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    b.tunnel.join_code(&code, false).await.unwrap();
    // While the first two are still comparing safety codes...
    let (safety_a, safety_b) = tokio::join!(a.wait_safety(), b.wait_safety());
    assert_eq!(safety_a, safety_b);

    // ...a third device with the same code gets nowhere: the meeting point is
    // gone, or it answers "in use".
    c.tunnel.join_code(&code, false).await.unwrap();
    let ended = c.wait_ended().await;
    assert!(
        matches!(ended.as_deref(), Some("code_in_use" | "code_not_found")),
        "{ended:?}"
    );

    // The first pair is unaffected.
    a.tunnel.confirm_safety(true).await.unwrap();
    b.tunnel.confirm_safety(true).await.unwrap();
    tokio::join!(a.wait_active(), b.wait_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_takes_the_code_off_the_network() {
    let relay = relay().await;
    let (mut a, mut b) = (
        serverless_node(&relay, "Alpha"),
        serverless_node(&relay, "Bravo"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    a.tunnel.end_session().await;
    a.wait_ended().await;

    b.tunnel.join_code(&code, false).await.unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_not_found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_any_relay_there_is_nowhere_to_meet() {
    // Relays off and no pairing server: the app must say so, not show a code
    // that can never work.
    let mut a = isolated_node("Alpha");
    a.tunnel.create_code().await.unwrap();
    let error = a
        .wait_session("the session to end", |s| match s {
            SessionState::ShowingCode { .. } => panic!("a code was shown with nowhere to meet"),
            SessionState::Ended { error, .. } => Some(error.as_ref().map(|e| e.code.to_string())),
            _ => None,
        })
        .await;
    assert_eq!(error.as_deref(), Some("rendezvous_unreachable"));
}

/// The real thing: two engines with default settings find each other through
/// the public relay network. Not part of `cargo test` because it needs the
/// internet and someone else's servers; CI runs it as a separate check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the internet: uses the public relay network"]
async fn default_settings_pair_two_devices_over_the_public_relays() {
    let (mut a, mut b) = (public_node("Alpha"), public_node("Bravo"));
    let started = std::time::Instant::now();
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    println!(
        "code on screen after {:.1} s",
        started.elapsed().as_secs_f64()
    );

    let joining = std::time::Instant::now();
    b.tunnel.join_code(&code, false).await.unwrap();
    let (safety_a, safety_b) = tokio::join!(a.wait_safety(), b.wait_safety());
    assert_eq!(safety_a, safety_b);
    println!("safety code after {:.1} s", joining.elapsed().as_secs_f64());
    a.tunnel.confirm_safety(true).await.unwrap();
    b.tunnel.confirm_safety(true).await.unwrap();
    tokio::join!(a.wait_active(), b.wait_active());

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("hello.bin");
    let body = pattern(3, 256 * 1024);
    std::fs::write(&file, &body).unwrap();
    let (_, received) = transfer(&mut a, &mut b, vec![file]).await;
    assert_eq!(received.state, TransferState::Done);
    assert_eq!(std::fs::read(b.saved("hello.bin")).unwrap(), body);

    // A wrong number finds nobody, and says so.
    let mut c = public_node("Charlie");
    let free = if code.starts_with("1000-") {
        1001
    } else {
        1000
    };
    c.tunnel
        .join_code(&format!("{free}-aardvark-abandoned-abbreviate"), false)
        .await
        .unwrap();
    assert_eq!(c.wait_ended().await.as_deref(), Some("code_not_found"));
}

// ---------------------------------------------------------------------------
// Strangers at the meeting point.
//
// The meeting point's key follows from the four public digits, so anyone can
// dial it and anyone can pretend to be it. These tests play that stranger with
// a bare endpoint built from the protocol crates only.
// ---------------------------------------------------------------------------

mod stranger {
    use std::time::Duration;

    use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
    use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl, SecretKey, TransportAddr};
    use tunnel_proto::frame;
    use tunnel_proto::hello::PairingMessage;
    use tunnel_proto::meeting;
    use tunnel_tests::LocalRelay;

    fn relay_url(relay: &LocalRelay) -> RelayUrl {
        relay.url.parse().unwrap()
    }

    /// A bare endpoint on the test relay. With `key`, it is an impostor
    /// holding a meeting point's own key.
    pub async fn endpoint(relay: &LocalRelay, key: Option<[u8; 32]>) -> Endpoint {
        let secret = match key {
            Some(bytes) => SecretKey::from_bytes(&bytes),
            None => SecretKey::generate(),
        };
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .alpns(vec![meeting::ALPN.to_vec()])
            .relay_mode(RelayMode::custom([relay_url(relay)]))
            .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
            .bind()
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), endpoint.online())
            .await
            .expect("reach the relay");
        endpoint
    }

    /// Dials the meeting point of `nameplate` and waits for its greeting.
    pub async fn dial(
        endpoint: &Endpoint,
        relay: &LocalRelay,
        nameplate: u16,
    ) -> (Connection, SendStream, RecvStream, PairingMessage) {
        let id = SecretKey::from_bytes(&tunnel_crypto::code::meeting_key(nameplate)).public();
        let addr = EndpointAddr::from_parts(id, [TransportAddr::Relay(relay_url(relay))]);
        let connection = endpoint.connect(addr, meeting::ALPN).await.unwrap();
        let (send, mut recv) = connection.accept_bi().await.unwrap();
        let greeting = read(&mut recv).await.expect("the host's first message");
        (connection, send, recv, greeting)
    }

    pub async fn write(send: &mut SendStream, message: &PairingMessage) {
        let bytes = frame::encode(message, meeting::MAX_FRAME).unwrap();
        send.write_all(&bytes).await.unwrap();
    }

    /// Reads one pairing frame, or `None` if the stream or connection ended.
    pub async fn read(recv: &mut RecvStream) -> Option<PairingMessage> {
        let mut header = [0u8; frame::HEADER_LEN];
        recv.read_exact(&mut header).await.ok()?;
        let len = frame::payload_len(header, meeting::MAX_FRAME).ok()?;
        let mut payload = vec![0u8; len];
        recv.read_exact(&mut payload).await.ok()?;
        frame::decode_payload(&payload).ok()
    }
}

fn nameplate_of(code: &str) -> u16 {
    code.split('-').next().unwrap().parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_stranger_does_not_keep_the_real_device_out() {
    let relay = relay().await;
    let (mut a, mut b) = (
        serverless_node(&relay, "Alpha"),
        serverless_node(&relay, "Bravo"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;

    // A stranger who only knows the number connects, is greeted, and stays
    // silent. The greeting is a SPAKE2 message: it tells them nothing.
    let lurker = stranger::endpoint(&relay, None).await;
    let (_connection, _send, _recv, greeting) =
        stranger::dial(&lurker, &relay, nameplate_of(&code)).await;
    assert!(matches!(
        greeting,
        tunnel_proto::hello::PairingMessage::Pake(_)
    ));

    // The real device still gets in, and the session opens.
    b.tunnel.join_code(&code, false).await.unwrap();
    let (safety_a, safety_b) = tokio::join!(a.wait_safety(), b.wait_safety());
    assert_eq!(safety_a, safety_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn garbage_from_a_stranger_does_not_use_up_the_code() {
    let relay = relay().await;
    let (mut a, mut b) = (
        serverless_node(&relay, "Alpha"),
        serverless_node(&relay, "Bravo"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;

    let vandal = stranger::endpoint(&relay, None).await;
    // An oversized length prefix, then noise that is not a pairing message.
    for junk in [
        vec![0xff, 0xff, 0xff, 0xff, 1, 2, 3],
        vec![0, 0, 0, 3, 0xff, 0xff, 0xff],
    ] {
        let (connection, mut send, _recv, _) =
            stranger::dial(&vandal, &relay, nameplate_of(&code)).await;
        send.write_all(&junk).await.unwrap();
        // The host drops the connection; nothing else happens.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), connection.closed())
            .await
            .expect("the host hung up on garbage");
    }

    // No guess was made, so the code still works.
    b.tunnel.join_code(&code, false).await.unwrap();
    let (safety_a, safety_b) = tokio::join!(a.wait_safety(), b.wait_safety());
    assert_eq!(safety_a, safety_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stranger_gets_one_guess_and_cannot_stall_to_get_more() {
    use tunnel_crypto::{ConnectionCode, PakeState, Role};
    use tunnel_proto::hello::PairingMessage;

    let relay = relay().await;
    let (mut a, mut b) = (
        serverless_node(&relay, "Alpha"),
        serverless_node(&relay, "Bravo"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;

    // The stranger guesses the words, sends a well-formed SPAKE2 message, and
    // then never confirms: it hopes to keep the code alive for another guess.
    let guess: ConnectionCode = wrong_words(&code).parse().unwrap();
    let guesser = stranger::endpoint(&relay, None).await;
    let (_connection, mut send, mut recv, _greeting) =
        stranger::dial(&guesser, &relay, nameplate_of(&code)).await;
    let (_state, message) = PakeState::start(Role::B, &guess);
    stranger::write(&mut send, &PairingMessage::Pake(message)).await;
    // The host answers with its confirmation, which only checks one guess.
    assert!(matches!(
        stranger::read(&mut recv).await,
        Some(PairingMessage::Confirm(_))
    ));

    // While the stranger stalls, nobody else can use the code...
    b.tunnel.join_code(&code, false).await.unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_in_use"));
    // ...and when the stranger never confirms, the code dies with the attempt.
    assert_eq!(a.wait_ended().await.as_deref(), Some("code_failed"));
    let mut c = serverless_node(&relay, "Charlie");
    c.tunnel.join_code(&code, false).await.unwrap();
    assert_eq!(c.wait_ended().await.as_deref(), Some("code_not_found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_impostor_at_the_meeting_point_cannot_open_a_session() {
    use tunnel_crypto::{ConnectionCode, PakeState, Role};
    use tunnel_proto::hello::PairingMessage;

    let relay = relay().await;
    let mut b = serverless_node(&relay, "Bravo");
    // The real code, known to Bravo but not to the impostor, who only saw the
    // number and so can compute the meeting point's key.
    let code = ConnectionCode::from_parts(4821, [100, 200, 300])
        .unwrap()
        .to_string();
    let impostor = stranger::endpoint(&relay, Some(tunnel_crypto::code::meeting_key(4821))).await;
    let guess: ConnectionCode = wrong_words(&code).parse().unwrap();

    let playing_host = tokio::spawn(async move {
        let connection = impostor.accept().await.unwrap().await.unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        let (_state, message) = PakeState::start(Role::A, &guess);
        stranger::write(&mut send, &PairingMessage::Pake(message)).await;
        // Bravo answers with its own SPAKE2 message and a confirmation that
        // the impostor cannot match.
        let pake = stranger::read(&mut recv).await;
        let confirm = stranger::read(&mut recv).await;
        stranger::write(&mut send, &PairingMessage::Confirm([7u8; 32])).await;
        let after = stranger::read(&mut recv).await;
        (pake, confirm, after)
    });

    b.tunnel.join_code(&code, false).await.unwrap();
    // Bravo never reaches the safety code: the session ends.
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_failed"));
    let (pake, confirm, after) = playing_host.await.unwrap();
    assert!(matches!(pake, Some(PairingMessage::Pake(_))));
    assert!(matches!(confirm, Some(PairingMessage::Confirm(_))));
    // No sealed hello (addresses, device name) was ever sent to the impostor.
    assert!(after.is_none(), "{after:?}");
}
