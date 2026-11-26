//! The hostile-peer harness: a fake peer attacks a real engine.
//!
//! Pass means: no file outside the destination folder, no panic, the session
//! either survives cleanly or is shut down, and memory stays bounded (every
//! oversized claim is refused before anything is allocated or stored).

use std::time::Duration;

use tunnel_core::{Event, TransferState};
use tunnel_proto::control::{CancelReason, ControlMessage, DeclineReason, Offer, TransferId};
use tunnel_proto::data::{DataHeader, Get, Refusal};
use tunnel_proto::manifest::{EntryKind, ManifestEntry, ManifestPage};
use tunnel_tests::evil::{Evil, Misbehave, serve};
use tunnel_tests::*;

const ID: TransferId = TransferId([0xEE; 16]);

fn file(id: u32, path: &[&str], size: u64) -> ManifestEntry {
    ManifestEntry {
        file_id: id,
        kind: EntryKind::File,
        path: path.iter().map(|s| s.to_string()).collect(),
        size,
        modified_ms: 1_700_000_000_000,
        executable: false,
    }
}

fn folder(id: u32, path: &[&str]) -> ManifestEntry {
    ManifestEntry {
        kind: EntryKind::Folder,
        ..file(id, path, 0)
    }
}

fn offer(id: TransferId, entries: &[ManifestEntry]) -> Offer {
    Offer {
        transfer_id: id,
        file_count: entries.iter().filter(|e| e.kind == EntryKind::File).count() as u64,
        folder_count: entries
            .iter()
            .filter(|e| e.kind == EntryKind::Folder)
            .count() as u64,
        total_bytes: entries.iter().map(|e| e.size).sum(),
        manifest_pages: 1,
    }
}

/// A victim showing a code, and an attacker who knows it and gets all the way
/// into an open session.
async fn victim_and_attacker() -> (LocalRendezvous, Node, Evil) {
    let rv = rendezvous().await;
    let mut victim = node(&rv.url, "Victim");
    victim.tunnel.create_code().await.unwrap();
    let code = victim.wait_code().await;
    let mut evil = Evil::join(&rv.url, &code, Misbehave::default())
        .await
        .expect("attacker pairs");
    victim.wait_safety().await;
    evil.confirm_safety().await;
    victim.tunnel.confirm_safety(true).await.unwrap();
    victim.wait_active().await;
    let confirmed = evil.recv().await;
    assert_eq!(confirmed, Some(ControlMessage::SafetyConfirmed));
    (rv, victim, evil)
}

async fn send_offer(evil: &mut Evil, id: TransferId, entries: Vec<ManifestEntry>) {
    evil.send(&ControlMessage::Offer(offer(id, &entries))).await;
    evil.send(&ControlMessage::ManifestPage(ManifestPage {
        transfer_id: id,
        page: 0,
        entries,
    }))
    .await;
}

async fn expect_blocked(victim: &mut Node, evil: &mut Evil, id: TransferId) {
    let declined = evil
        .recv_until(|m| match m {
            ControlMessage::Decline {
                transfer_id,
                reason,
            } if *transfer_id == id => Some(*reason),
            _ => None,
        })
        .await;
    assert_eq!(declined, Some(DeclineReason::Invalid));
    victim
        .wait("the blocked notice", |e| {
            matches!(e, Event::OfferBlocked { .. }).then_some(())
        })
        .await;
    assert!(
        victim.tunnel.snapshot().offers.is_empty(),
        "a blocked offer is never shown"
    );
}

fn assert_nothing_saved(victim: &Node) {
    assert!(
        tree(victim.save.path()).is_empty(),
        "files were written: {:?}",
        tree(victim.save.path())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn traversal_and_separator_names_reject_the_whole_offer() {
    let cases: Vec<Vec<ManifestEntry>> = vec![
        vec![file(0, &["..", "..", "evil.txt"], 4)],
        vec![file(0, &[".."], 4)],
        vec![file(0, &["."], 4)],
        vec![file(0, &["../../../../etc/cron.d/evil"], 4)],
        vec![file(0, &["/etc/passwd"], 4)],
        vec![file(0, &["a/b"], 4)],
        vec![file(0, &["nul\0byte"], 4)],
        vec![file(0, &[""], 4)],
        vec![
            folder(0, &["ok"]),
            file(1, &["ok", "..", "..", "escape"], 4),
        ],
        // A file inside a folder that was never listed, and inside a "folder" that is a file.
        vec![file(0, &["ghost", "x"], 4)],
        vec![file(0, &["a"], 1), file(1, &["a", "b"], 3)],
    ];
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    for (i, entries) in cases.into_iter().enumerate() {
        let id = TransferId([i as u8 + 1; 16]);
        send_offer(&mut evil, id, entries).await;
        expect_blocked(&mut victim, &mut evil, id).await;
    }
    assert_nothing_saved(&victim);
    // The session itself is still healthy.
    evil.send(&ControlMessage::Text {
        text: "still here".into(),
    })
    .await;
    assert_eq!(victim.wait_text().await, "still here");

    // An empty path, or one more than 64 levels deep, breaks a wire limit: the
    // message itself is invalid and the session is closed.
    for entries in [vec![file(0, &[], 4)], vec![file(0, &["d"; 65], 4)]] {
        let (_rv, mut victim, mut evil) = victim_and_attacker().await;
        send_offer(&mut evil, ID, entries).await;
        assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));
        assert_nothing_saved(&victim);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn totals_and_page_counts_that_disagree_are_rejected() {
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;

    // The offer claims more than the manifest holds.
    let id1 = TransferId([1; 16]);
    let entries = vec![file(0, &["a.txt"], 5)];
    let mut lying = offer(id1, &entries);
    lying.total_bytes = 5_000;
    evil.send(&ControlMessage::Offer(lying)).await;
    evil.send(&ControlMessage::ManifestPage(ManifestPage {
        transfer_id: id1,
        page: 0,
        entries,
    }))
    .await;
    expect_blocked(&mut victim, &mut evil, id1).await;

    // File IDs out of order.
    let id2 = TransferId([2; 16]);
    send_offer(&mut evil, id2, vec![file(1, &["a.txt"], 5)]).await;
    expect_blocked(&mut victim, &mut evil, id2).await;

    // More entries in the page than the offer announced.
    let id3 = TransferId([3; 16]);
    let entries = vec![file(0, &["a"], 1), file(1, &["b"], 1), file(2, &["c"], 1)];
    let mut small = offer(id3, &entries[..1]);
    small.manifest_pages = 1;
    evil.send(&ControlMessage::Offer(small)).await;
    evil.send(&ControlMessage::ManifestPage(ManifestPage {
        transfer_id: id3,
        page: 0,
        entries,
    }))
    .await;
    expect_blocked(&mut victim, &mut evil, id3).await;

    // A page with the wrong page number.
    let id4 = TransferId([4; 16]);
    let entries = vec![file(0, &["a"], 1)];
    evil.send(&ControlMessage::Offer(offer(id4, &entries)))
        .await;
    evil.send(&ControlMessage::ManifestPage(ManifestPage {
        transfer_id: id4,
        page: 7,
        entries,
    }))
    .await;
    expect_blocked(&mut victim, &mut evil, id4).await;
    assert_nothing_saved(&victim);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ten_million_entries_and_a_petabyte_are_refused_up_front() {
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;

    // 10 million entries: over the receiver's limit, refused before any page.
    let huge = Offer {
        transfer_id: ID,
        file_count: 10_000_000,
        folder_count: 0,
        total_bytes: 1,
        manifest_pages: 1000,
    };
    evil.send(&ControlMessage::Offer(huge)).await;
    expect_blocked(&mut victim, &mut evil, ID).await;

    // More pages than entries can never be honest.
    let id2 = TransferId([2; 16]);
    let padded = Offer {
        transfer_id: id2,
        file_count: 2,
        folder_count: 0,
        total_bytes: 2,
        manifest_pages: 50,
    };
    evil.send(&ControlMessage::Offer(padded)).await;
    expect_blocked(&mut victim, &mut evil, id2).await;

    // A fake 1 PB file: the offer is shown, but Accept is refused for lack of space.
    let id3 = TransferId([3; 16]);
    send_offer(&mut evil, id3, vec![file(0, &["huge.bin"], 1 << 40)]).await;
    let shown = victim.wait_offer().await;
    assert!(shown.shortfall > 0, "the shortfall is shown on the offer");
    let refused = victim
        .tunnel
        .respond_offer(&shown.id, true, None)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "no_space");
    assert_nothing_saved(&victim);

    // A single file over the 1 TiB limit breaks a field limit: the session closes.
    let id4 = TransferId([4; 16]);
    send_offer(
        &mut evil,
        id4,
        vec![file(0, &["absurd.bin"], (1 << 40) + 1)],
    )
    .await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn more_than_four_waiting_offers_are_declined_as_busy() {
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    for i in 0..4u8 {
        send_offer(
            &mut evil,
            TransferId([i + 1; 16]),
            vec![file(0, &["a.txt"], 1)],
        )
        .await;
        victim.wait_offer().await;
    }
    let fifth = TransferId([9; 16]);
    send_offer(&mut evil, fifth, vec![file(0, &["a.txt"], 1)]).await;
    let declined = evil
        .recv_until(|m| match m {
            ControlMessage::Decline {
                transfer_id,
                reason,
            } if *transfer_id == fifth => Some(*reason),
            _ => None,
        })
        .await;
    assert_eq!(declined, Some(DeclineReason::Busy));
    assert_eq!(victim.tunnel.snapshot().offers.len(), 4);

    // Reusing a transfer ID is never legitimate.
    send_offer(
        &mut evil,
        TransferId([1; 16]),
        vec![file(0, &["again.txt"], 1)],
    )
    .await;
    let declined = evil
        .recv_until(|m| match m {
            ControlMessage::Decline { reason, .. } => Some(*reason),
            _ => None,
        })
        .await;
    assert_eq!(declined, Some(DeclineReason::Invalid));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_and_garbage_frames_close_the_session() {
    // A frame that claims to be 2 MiB (limit: 1 MiB).
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    evil.send_raw(&(2 * 1024 * 1024u32).to_be_bytes()).await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));
    assert!(evil.closed().await);

    // A frame that claims to be 4 GiB.
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    evil.send_raw(&[0xff, 0xff, 0xff, 0xff]).await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));

    // An unknown message type.
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    evil.send_raw(&[0, 0, 0, 2, 200, 1]).await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));

    // A text message over 64 KiB.
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    evil.send(&ControlMessage::Text {
        text: "x".repeat(70_000),
    })
    .await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));

    // A second Bind, long after the first.
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    evil.send(&ControlMessage::Bind { mac: [0; 32] }).await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_accepted_before_the_safety_code_is_confirmed() {
    let rv = rendezvous().await;
    let mut victim = node(&rv.url, "Victim");
    victim.tunnel.create_code().await.unwrap();
    let code = victim.wait_code().await;
    let mut evil = Evil::join(&rv.url, &code, Misbehave::default())
        .await
        .unwrap();
    victim.wait_safety().await;
    // The attacker skips the safety step and goes straight to an offer.
    send_offer(&mut evil, ID, vec![file(0, &["early.txt"], 1)]).await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("protocol"));
    assert_nothing_saved(&victim);

    // Requests for files are also refused before the session opens.
    let mut victim = node(&rv.url, "Victim");
    victim.tunnel.create_code().await.unwrap();
    let code = victim.wait_code().await;
    let mut evil = Evil::join(&rv.url, &code, Misbehave::default())
        .await
        .unwrap();
    victim.wait_safety().await;
    let answer = evil
        .get(Get {
            transfer_id: ID,
            file_id: 0,
            offset: 0,
        })
        .await;
    assert_eq!(
        answer.map(|(header, _)| header),
        Some(DataHeader::Refused(Refusal::NotAccepted))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_after_cancel_are_ignored() {
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    let entries = vec![file(0, &["a.txt"], 1)];
    evil.send(&ControlMessage::Offer(offer(ID, &entries))).await;
    evil.send(&ControlMessage::Cancel {
        transfer_id: ID,
        reason: CancelReason::User,
    })
    .await;
    // Pages, accepts and completions for a cancelled or unknown transfer.
    evil.send(&ControlMessage::ManifestPage(ManifestPage {
        transfer_id: ID,
        page: 0,
        entries,
    }))
    .await;
    evil.send(&ControlMessage::Accept { transfer_id: ID }).await;
    evil.send(&ControlMessage::Unpause {
        transfer_id: TransferId([7; 16]),
    })
    .await;
    evil.send(&ControlMessage::Cancel {
        transfer_id: TransferId([8; 16]),
        reason: CancelReason::User,
    })
    .await;
    evil.send(&ControlMessage::Text {
        text: "ping".into(),
    })
    .await;
    assert_eq!(victim.wait_text().await, "ping");
    assert!(victim.tunnel.snapshot().offers.is_empty());
    assert_nothing_saved(&victim);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserved_and_deceptive_names_land_safely_inside_the_destination() {
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    let body = b"payload!";
    let entries = vec![
        file(0, &["CON"], 8),
        file(1, &["invoice\u{202e}fdp.exe"], 8),
        file(2, &["..\\..\\Windows\\System32\\x.dll"], 8),
        file(3, &["C:\\autoexec.bat"], 8),
        file(4, &["trailing. . "], 8),
        folder(5, &["nul.d"]),
        file(6, &["nul.d", "LPT1.txt"], 8),
    ];
    send_offer(&mut evil, ID, entries).await;
    let shown = victim.wait_offer().await;
    let warnings = serde_json::to_string(&shown.warnings).unwrap();
    assert!(warnings.contains("bidi_names"), "{warnings}");
    assert!(warnings.contains("renamed"), "{warnings}");
    assert!(warnings.contains("executables"), "{warnings}");
    victim
        .tunnel
        .respond_offer(&shown.id, true, None)
        .await
        .unwrap();

    let hash = *blake3::hash(body).as_bytes();
    for _ in 0..6 {
        let (mut send, _recv, get) = evil
            .next_get()
            .await
            .expect("the victim asks for each file");
        assert_eq!(get.offset, 0);
        serve(
            &mut send,
            DataHeader::Ok { size: 8, offset: 0 },
            body,
            &hash,
            &[],
        )
        .await;
    }
    let done = victim.wait_transfer_end(&shown.id).await;
    assert_eq!(done.state, TransferState::Done);
    assert_eq!(
        (done.files_done, done.files_failed),
        (6, 0),
        "{:?}",
        done.issues
    );

    let mut names: Vec<String> = tree(victim.save.path())
        .into_iter()
        .map(|(rel, _)| rel)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            ".._.._Windows_System32_x.dll",
            "CON_",
            "C__autoexec.bat",
            "invoice_fdp.exe",
            "nul_.d/",
            "nul_.d/LPT1_.txt",
            "trailing____",
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_file_data_never_reaches_the_destination() {
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    let body = b"0123456789";
    let good = *blake3::hash(body).as_bytes();
    let entries = vec![
        file(0, &["wrong-hash.bin"], 10),
        file(1, &["extra-bytes.bin"], 10),
        file(2, &["size-lie.bin"], 10),
        file(3, &["short.bin"], 10),
        file(4, &["offset-lie.bin"], 10),
        file(5, &["honest.bin"], 10),
    ];
    send_offer(&mut evil, ID, entries).await;
    let shown = victim.wait_offer().await;
    victim
        .tunnel
        .respond_offer(&shown.id, true, None)
        .await
        .unwrap();

    let mut wrong_hash_attempts = 0;
    let mut honest_served = false;
    // Each bad file is asked for once, except the wrong hash (retried once).
    for _ in 0..7 {
        let Some((mut send, _recv, get)) = evil.next_get().await else {
            break;
        };
        let ok = DataHeader::Ok {
            size: 10,
            offset: 0,
        };
        match get.file_id {
            0 => {
                wrong_hash_attempts += 1;
                serve(&mut send, ok, body, &[0u8; 32], &[]).await;
            }
            1 => serve(&mut send, ok, body, &good, b"trailing garbage").await,
            2 => {
                serve(
                    &mut send,
                    DataHeader::Ok {
                        size: 1_000_000,
                        offset: 0,
                    },
                    body,
                    &good,
                    &[],
                )
                .await
            }
            3 => serve(&mut send, ok, &body[..4], &[], &[]).await,
            4 => {
                serve(
                    &mut send,
                    DataHeader::Ok {
                        size: 10,
                        offset: 5,
                    },
                    &body[5..],
                    &good,
                    &[],
                )
                .await
            }
            5 => {
                honest_served = true;
                serve(&mut send, ok, body, &good, &[]).await;
            }
            other => panic!("unexpected file {other}"),
        }
    }
    let done = victim.wait_transfer_end(&shown.id).await;
    assert_eq!(
        wrong_hash_attempts, 2,
        "a hash mismatch is retried exactly once"
    );
    assert!(honest_served);
    assert_eq!(done.state, TransferState::Done);
    assert_eq!(
        (done.files_done, done.files_failed),
        (1, 5),
        "{:?}",
        done.issues
    );
    assert_eq!(
        tree(victim.save.path()),
        [("honest.bin".to_string(), 10)],
        "only the verified file exists"
    );
    let reasons: std::collections::HashMap<&str, &str> = done
        .issues
        .iter()
        .map(|i| (i.path.as_str(), i.reason))
        .collect();
    assert_eq!(reasons["wrong-hash.bin"], "hash_mismatch");
    assert_eq!(reasons["extra-bytes.bin"], "protocol");
    assert_eq!(reasons["size-lie.bin"], "protocol");
    assert_eq!(reasons["offset-lie.bin"], "protocol");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fake_receiver_gets_only_what_was_accepted() {
    let (_rv, mut victim, mut evil) = victim_and_attacker().await;
    let src = tempfile::tempdir().unwrap();
    write_file(&src.path().join("secret/one.txt"), b"first");
    write_file(&src.path().join("secret/two.txt"), b"second!");
    let id_hex = victim
        .tunnel
        .send_paths(vec![src.path().join("secret")])
        .await
        .unwrap();
    let id = TransferId::from_hex(&id_hex).unwrap();
    let offered = evil
        .recv_until(|m| match m {
            ControlMessage::ManifestPage(page) => Some(page.entries.clone()),
            _ => None,
        })
        .await
        .expect("manifest");
    assert_eq!(offered.len(), 3);
    let refused = |refusal| Some(DataHeader::Refused(refusal));
    let header = |answer: Option<(DataHeader, usize)>| answer.map(|(header, _)| header);

    // Before accepting: nothing, for this transfer or any made-up one.
    assert_eq!(
        header(
            evil.get(Get {
                transfer_id: id,
                file_id: 1,
                offset: 0
            })
            .await
        ),
        refused(Refusal::NotAccepted)
    );
    assert_eq!(
        header(
            evil.get(Get {
                transfer_id: ID,
                file_id: 0,
                offset: 0
            })
            .await
        ),
        refused(Refusal::NotAccepted)
    );

    evil.send(&ControlMessage::Accept { transfer_id: id }).await;
    victim
        .wait_transfer_state(&id_hex, TransferState::Active)
        .await;
    // After accepting: only real files, only valid offsets.
    assert_eq!(
        header(
            evil.get(Get {
                transfer_id: id,
                file_id: 0,
                offset: 0
            })
            .await
        ),
        refused(Refusal::UnknownFile)
    );
    assert_eq!(
        header(
            evil.get(Get {
                transfer_id: id,
                file_id: 99,
                offset: 0
            })
            .await
        ),
        refused(Refusal::UnknownFile)
    );
    assert_eq!(
        header(
            evil.get(Get {
                transfer_id: id,
                file_id: 1,
                offset: 9999
            })
            .await
        ),
        refused(Refusal::BadOffset)
    );
    let file_id = offered
        .iter()
        .find(|e| e.path.last().unwrap() == "two.txt")
        .unwrap()
        .file_id;
    let (ok, body_len) = evil
        .get(Get {
            transfer_id: id,
            file_id,
            offset: 3,
        })
        .await
        .unwrap();
    assert_eq!(ok, DataHeader::Ok { size: 7, offset: 3 });
    assert_eq!(
        body_len,
        4 + 32,
        "the bytes from the offset, then the 32-byte hash"
    );

    // After cancelling: nothing again.
    victim.tunnel.cancel(&id_hex).await.unwrap();
    assert_eq!(
        header(
            evil.get(Get {
                transfer_id: id,
                file_id,
                offset: 0
            })
            .await
        ),
        refused(Refusal::NotAccepted)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tampered_pairing_is_caught() {
    let rv = rendezvous().await;
    let pairing = |misbehave: Misbehave| {
        let url = rv.url.clone();
        async move {
            let mut victim = node(&url, "Victim");
            victim.tunnel.create_code().await.unwrap();
            let code = victim.wait_code().await;
            let attempt = Evil::join(&url, &code, misbehave).await;
            (victim, attempt.err())
        }
    };

    // A wrong key-confirmation tag: "That code didn't work."
    let (mut victim, _) = pairing(Misbehave {
        wrong_confirmation: true,
        ..Default::default()
    })
    .await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("code_failed"));

    // The host's own SPAKE2 message replayed back at it.
    let (mut victim, _) = pairing(Misbehave {
        reflect_pake: true,
        ..Default::default()
    })
    .await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("code_failed"));

    // The host's own confirmation tag replayed back at it.
    let (mut victim, _) = pairing(Misbehave {
        reflect_confirmation: true,
        ..Default::default()
    })
    .await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("code_failed"));

    // A wrong channel-binding tag: the connection is not from the party that knew the code.
    let (mut victim, attacker_error) = pairing(Misbehave {
        wrong_bind: true,
        ..Default::default()
    })
    .await;
    assert_eq!(victim.wait_ended().await.as_deref(), Some("bind_failed"));
    assert!(
        attacker_error.is_some(),
        "the attacker never receives the host's Bind"
    );

    // No protocol version in common.
    let (mut victim, _) = pairing(Misbehave {
        versions: Some(vec![99]),
        ..Default::default()
    })
    .await;
    assert_eq!(
        victim.wait_ended().await.as_deref(),
        Some("version_mismatch")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_code_gets_exactly_one_guess() {
    let rv = rendezvous().await;
    let mut victim = node(&rv.url, "Victim");
    victim.tunnel.create_code().await.unwrap();
    let code = victim.wait_code().await;
    let nameplate = code.split('-').next().unwrap();
    let guess = format!("{nameplate}-aardvark-abandoned-abbreviate");
    assert!(
        Evil::join(&rv.url, &guess, Misbehave::default())
            .await
            .is_err()
    );
    assert_eq!(victim.wait_ended().await.as_deref(), Some("code_failed"));
    // The room is gone: a second guess, even the right one, finds nothing.
    let second = Evil::join(&rv.url, &code, Misbehave::default()).await;
    assert!(second.is_err());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rv.server.stats().rooms, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hostile_device_name_is_made_safe_to_show() {
    let rv = rendezvous().await;
    let mut victim = node(&rv.url, "Victim");
    victim.tunnel.create_code().await.unwrap();
    let code = victim.wait_code().await;
    let name = "Mom's \u{202e}enohP\n<script>".to_string();
    let _evil = Evil::join(
        &rv.url,
        &code,
        Misbehave {
            device_name: Some(name),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    victim.wait_safety().await;
    match victim.tunnel.snapshot().session {
        tunnel_core::SessionState::VerifySafety { peer, .. } => {
            assert_eq!(peer.name, "Mom's enohP<script>")
        }
        other => panic!("{other:?}"),
    }
}
