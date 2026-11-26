//! Two real engines, a local rendezvous server, loopback networking.

use std::time::Duration;

use tunnel_core::{ConflictRule, Event, PathKind, SessionState, TransferState};
use tunnel_tests::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_and_exchange_text_both_ways() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    match a.tunnel.snapshot().session {
        SessionState::Active { peer, .. } => {
            assert_eq!(peer.name, "Bravo");
            assert_eq!(peer.device_type, "desktop");
        }
        other => panic!("unexpected state {other:?}"),
    }

    a.tunnel.send_text("hello from alpha".into()).await.unwrap();
    assert_eq!(b.wait_text().await, "hello from alpha");
    b.tunnel
        .send_text("https://example.com/a?b=c".into())
        .await
        .unwrap();
    assert_eq!(a.wait_text().await, "https://example.com/a?b=c");

    assert!(
        a.tunnel.send_text("x".repeat(70_000)).await.is_err(),
        "text over 64 KiB is refused"
    );
    assert!(a.tunnel.send_text("   ".into()).await.is_err());

    // The room is gone as soon as pairing is over.
    assert_eq!(rv.server.stats().rooms, 0);

    a.tunnel.end_session().await;
    assert_eq!(
        b.wait_ended().await,
        None,
        "the other side sees a clean goodbye"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_file_arrives_intact_with_its_modified_time() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("report.bin");
    write_file(&file, &pattern(1, 3 * 1024 * 1024 + 123));
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(old)
        .unwrap();

    let (sent, received) = transfer(&mut a, &mut b, vec![file.clone()]).await;
    assert_eq!(received.state, TransferState::Done);
    assert_eq!(
        (
            received.files_done,
            received.files_failed,
            received.files_skipped
        ),
        (1, 0, 0)
    );
    assert_eq!(received.bytes_done, received.bytes_total);
    assert_eq!(sent.state, TransferState::Done);
    assert_eq!(sent.files_done, 1);

    let saved = b.saved("report.bin");
    assert_eq!(hash_file(&saved), hash_file(&file));
    assert_eq!(std::fs::metadata(&saved).unwrap().modified().unwrap(), old);
    assert_eq!(
        tree(b.save.path()),
        [("report.bin".to_string(), 3 * 1024 * 1024 + 123)],
        "no partial folder is left"
    );

    let history = b.tunnel.history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        (history[0].direction.as_str(), history[0].result.as_str()),
        ("receive", "done")
    );
    assert_eq!(history[0].peer, "Alpha");
    assert_eq!(a.tunnel.history(10).unwrap()[0].direction, "send");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn folder_tree_with_empty_folders_zero_byte_files_and_unicode_names() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("Projet été");
    write_file(&root.join("LISEZ-MOI.txt"), b"bonjour");
    write_file(&root.join("vide.dat"), b"");
    write_file(&root.join("données/résumé.txt"), "accentué".as_bytes());
    write_file(&root.join("中文/文件.txt"), "你好".as_bytes());
    write_file(&root.join("emoji/📷 photo 🏖️.bin"), &pattern(2, 70_000));
    write_file(
        &root.join("deep/a/b/c/d/e/file.bin"),
        &pattern(3, 1_000_000),
    );
    std::fs::create_dir_all(root.join("empty/inner")).unwrap();

    let (sent, received) = transfer(&mut a, &mut b, vec![root.clone()]).await;
    assert_eq!(received.state, TransferState::Done, "{received:?}");
    assert_eq!(
        received.files_failed + received.files_skipped,
        0,
        "{:?}",
        received.issues
    );
    assert_eq!(sent.state, TransferState::Done);

    let expected: Vec<(String, u64)> = tree(&root)
        .into_iter()
        .map(|(rel, size)| (format!("Projet été/{rel}"), size))
        .collect();
    let mut got = tree(b.save.path());
    got.retain(|(rel, _)| rel != "Projet été/");
    assert_eq!(got, expected);
    assert_eq!(
        hash_file(&b.saved("Projet été/emoji/📷 photo 🏖️.bin")),
        hash_file(&root.join("emoji/📷 photo 🏖️.bin"))
    );
    assert!(
        b.saved("Projet été/empty/inner").is_dir(),
        "empty folders are created as sent"
    );
    assert_eq!(
        std::fs::metadata(b.saved("Projet été/vide.dat"))
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thousands_of_small_files() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("many");
    let count = 3000;
    for i in 0..count {
        write_file(
            &root.join(format!("d{}/file-{i:05}.txt", i % 30)),
            format!("file number {i}").as_bytes(),
        );
    }
    let (_, received) = transfer(&mut a, &mut b, vec![root.clone()]).await;
    assert_eq!(received.state, TransferState::Done);
    assert_eq!((received.files_done, received.files_failed), (count, 0));
    let got = tree(&b.saved("many"));
    assert_eq!(got, tree(&root));
    assert_eq!(
        std::fs::read_to_string(b.saved("many/d7/file-00007.txt")).unwrap(),
        "file number 7"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_directions_at_once() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    write_file(&src.path().join("from-a.bin"), &pattern(10, 2_000_000));
    write_file(&src.path().join("from-b.bin"), &pattern(11, 2_500_000));

    let id_a = a
        .tunnel
        .send_paths(vec![src.path().join("from-a.bin")])
        .await
        .unwrap();
    let id_b = b
        .tunnel
        .send_paths(vec![src.path().join("from-b.bin")])
        .await
        .unwrap();
    let offer_at_b = b.wait_offer().await;
    let offer_at_a = a.wait_offer().await;
    assert_eq!(
        (offer_at_b.id.as_str(), offer_at_a.id.as_str()),
        (id_a.as_str(), id_b.as_str())
    );
    b.tunnel.respond_offer(&id_a, true, None).await.unwrap();
    a.tunnel.respond_offer(&id_b, true, None).await.unwrap();
    assert_eq!(b.wait_transfer_end(&id_a).await.state, TransferState::Done);
    assert_eq!(a.wait_transfer_end(&id_b).await.state, TransferState::Done);
    assert_eq!(
        hash_file(&b.saved("from-a.bin")),
        hash_file(&src.path().join("from-a.bin"))
    );
    assert_eq!(
        hash_file(&a.saved("from-b.bin")),
        hash_file(&src.path().join("from-b.bin"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offer_shows_names_and_warnings_and_nothing_is_written_before_accept() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    write_file(&src.path().join("invoice.pdf.exe"), b"MZ");
    write_file(&src.path().join("setup.sh"), b"#!/bin/sh");
    write_file(&src.path().join("notes.txt"), b"notes");
    let paths = ["invoice.pdf.exe", "setup.sh", "notes.txt"]
        .map(|n| src.path().join(n))
        .to_vec();

    let id = a.tunnel.send_paths(paths).await.unwrap();
    let offer = b.wait_offer().await;
    assert_eq!((offer.files, offer.folders, offer.bytes), (3, 0, 2 + 9 + 5));
    assert_eq!(offer.peer.name, "Alpha");
    let mut names = offer.names.clone();
    names.sort();
    assert_eq!(names, ["invoice.pdf.exe", "notes.txt", "setup.sh"]);
    let warnings = serde_json::to_string(&offer.warnings).unwrap();
    assert!(
        warnings.contains(r#""kind":"executables","count":2"#),
        "{warnings}"
    );
    assert!(
        warnings.contains(r#""kind":"double_extensions","count":1"#),
        "{warnings}"
    );

    // R8: no byte touches the receiver's disk before the receiver accepts.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tree(b.save.path()).is_empty());

    b.tunnel.respond_offer(&id, false, None).await.unwrap();
    let sent = a.wait_transfer_end(&id).await;
    assert_eq!(sent.state, TransferState::Declined);
    assert!(tree(b.save.path()).is_empty());
    assert!(
        b.tunnel.respond_offer(&id, true, None).await.is_err(),
        "a declined offer cannot be accepted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conflict_rules_keep_both_replace_and_skip() {
    for (rule, expect) in [
        (
            ConflictRule::KeepBoth,
            vec![("a (2).txt", "new"), ("a.txt", "old")],
        ),
        (ConflictRule::Replace, vec![("a.txt", "new")]),
        (ConflictRule::Skip, vec![("a.txt", "old")]),
    ] {
        let rv = rendezvous().await;
        let mut a = node(&rv.url, "Alpha");
        let mut b = node_with(&rv.url, "Bravo", |s| s.conflict_rule = rule);
        pair(&mut a, &mut b).await;
        write_file(&b.saved("a.txt"), b"old");
        let src = tempfile::tempdir().unwrap();
        write_file(&src.path().join("a.txt"), b"new");

        let (_, received) = transfer(&mut a, &mut b, vec![src.path().join("a.txt")]).await;
        assert_eq!(received.state, TransferState::Done);
        let mut got: Vec<(String, String)> = tree(b.save.path())
            .into_iter()
            .map(|(rel, _)| (rel.clone(), std::fs::read_to_string(b.saved(&rel)).unwrap()))
            .collect();
        got.sort();
        let expect: Vec<(String, String)> = expect
            .into_iter()
            .map(|(n, c)| (n.into(), c.into()))
            .collect();
        assert_eq!(got, expect, "{rule:?} {received:?}");
        if rule == ConflictRule::Skip {
            assert_eq!(received.files_skipped, 1);
            assert_eq!(received.issues[0].reason, "exists");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn names_unsafe_on_windows_are_renamed_and_listed() {
    if cfg!(windows) {
        return; // These names cannot exist on the sending side on Windows.
    }
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("odd");
    write_file(&root.join("CON"), b"1");
    write_file(&root.join("a:b?.txt"), b"2");
    write_file(&root.join("trailing."), b"3");
    write_file(&root.join("Readme.md"), b"4");
    write_file(&root.join("README.md"), b"5");

    let id = a.tunnel.send_paths(vec![root]).await.unwrap();
    let offer = b.wait_offer().await;
    assert!(
        serde_json::to_string(&offer.warnings)
            .unwrap()
            .contains(r#""kind":"renamed","count":4"#)
    );
    b.tunnel.respond_offer(&id, true, None).await.unwrap();
    let received = b.wait_transfer_end(&id).await;
    assert_eq!(received.state, TransferState::Done);
    let names: Vec<String> = tree(&b.saved("odd"))
        .into_iter()
        .map(|(rel, _)| rel)
        .collect();
    assert_eq!(names.len(), 5);
    for expected in ["CON_", "a_b_.txt", "trailing_"] {
        assert!(
            names.iter().any(|n| n == expected),
            "{expected} missing from {names:?}"
        );
    }
    assert!(
        names.iter().any(|n| n.contains(" (2)")),
        "case collision gets a number: {names:?}"
    );
    let renamed = received
        .issues
        .iter()
        .filter(|i| i.kind == "renamed")
        .count();
    assert_eq!(renamed, 4, "{:?}", received.issues);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_mid_transfer_leaves_only_complete_files() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("big");
    // Sixteen sparse 64 MiB files: quick to create, slow enough to interrupt.
    let size: u64 = 64 * 1024 * 1024;
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..16 {
        std::fs::File::create(root.join(format!("part{i:02}.bin")))
            .unwrap()
            .set_len(size)
            .unwrap();
    }
    let id = a.tunnel.send_paths(vec![root.clone()]).await.unwrap();
    b.wait_offer().await;
    b.tunnel.respond_offer(&id, true, None).await.unwrap();
    b.wait("some progress", |event| match event {
        Event::Transfer { transfer }
            if transfer.id == id && transfer.bytes_done > 4 * 1024 * 1024 =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    b.tunnel.cancel(&id).await.unwrap();
    assert_eq!(
        b.wait_transfer_end(&id).await.state,
        TransferState::Cancelled
    );
    assert_eq!(
        a.wait_transfer_end(&id).await.state,
        TransferState::Cancelled
    );

    // Whatever is in the folder is complete and verified; nothing partial.
    let left = tree(b.save.path());
    for (rel, size) in left.clone() {
        assert!(
            !rel.contains(".tunnel-partial"),
            "partial folder left behind: {left:?}"
        );
        if !rel.ends_with('/') {
            assert_eq!(size, 64 * 1024 * 1024);
            assert_eq!(
                hash_file(&b.saved(&rel)),
                hash_file(&root.join(rel.trim_start_matches("big/")))
            );
        }
    }
    let saved = tree(b.save.path())
        .into_iter()
        .filter(|(rel, _)| !rel.ends_with('/'))
        .count();
    assert!(saved < 16, "the transfer was cancelled before it finished");
    // The session is still usable afterwards.
    a.tunnel.send_text("still here".into()).await.unwrap();
    assert_eq!(b.wait_text().await, "still here");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume_from_either_side() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("pausable");
    for i in 0..5 {
        write_file(
            &root.join(format!("f{i}.bin")),
            &pattern(40 + i, 20 * 1024 * 1024),
        );
    }
    let id = a.tunnel.send_paths(vec![root.clone()]).await.unwrap();
    b.wait_offer().await;
    b.tunnel.respond_offer(&id, true, None).await.unwrap();
    let progressed = |min: u64| {
        let id = id.clone();
        move |event: &Event| match event {
            Event::Transfer { transfer } if transfer.id == id && transfer.bytes_done > min => {
                Some(())
            }
            _ => None,
        }
    };
    b.wait("progress", progressed(2 * 1024 * 1024)).await;

    // Receiver pauses, then resumes.
    b.tunnel.pause(&id).await.unwrap();
    b.wait_transfer_state(&id, TransferState::Paused).await;
    a.wait_transfer_state(&id, TransferState::Paused).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    b.tunnel.resume(&id).await.unwrap();
    b.wait_transfer_state(&id, TransferState::Active).await;

    // Sender pauses, then resumes.
    a.tunnel.pause(&id).await.unwrap();
    b.wait_transfer_state(&id, TransferState::Paused).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    a.tunnel.resume(&id).await.unwrap();

    let received = b.wait_transfer_end(&id).await;
    assert_eq!(received.state, TransferState::Done, "{:?}", received.issues);
    assert_eq!((received.files_done, received.files_failed), (5, 0));
    for i in 0..5 {
        let name = format!("f{i}.bin");
        assert_eq!(
            hash_file(&b.saved(&format!("pausable/{name}"))),
            hash_file(&root.join(&name))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cut_connection_reconnects_and_the_transfer_resumes() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("resumable");
    // Big enough that the cut always lands mid-transfer: one patterned file to
    // prove the bytes survive, plus sparse ones for bulk.
    write_file(&root.join("f0.bin"), &pattern(60, 32 * 1024 * 1024));
    for i in 1..8 {
        std::fs::File::create(root.join(format!("f{i}.bin")))
            .unwrap()
            .set_len(96 * 1024 * 1024)
            .unwrap();
    }
    let id = a.tunnel.send_paths(vec![root.clone()]).await.unwrap();
    b.wait_offer().await;
    b.tunnel.respond_offer(&id, true, None).await.unwrap();
    b.wait("progress", |event| match event {
        Event::Transfer { transfer }
            if transfer.id == id && transfer.bytes_done > 8 * 1024 * 1024 =>
        {
            Some(())
        }
        _ => None,
    })
    .await;

    // Cut the connection the way a network failure would.
    a.tunnel.debug_drop_connection().await;
    b.wait_session("reconnecting", |s| {
        matches!(s, SessionState::Reconnecting { .. }).then_some(())
    })
    .await;
    b.wait_active().await;

    let received = b.wait_transfer_end(&id).await;
    assert_eq!(received.state, TransferState::Done, "{:?}", received.issues);
    assert_eq!((received.files_done, received.files_failed), (8, 0));
    assert_eq!(a.wait_transfer_end(&id).await.state, TransferState::Done);
    for i in 0..8 {
        let name = format!("f{i}.bin");
        assert_eq!(
            hash_file(&b.saved(&format!("resumable/{name}"))),
            hash_file(&root.join(&name))
        );
    }
    // And the session keeps working.
    b.tunnel.send_text("made it".into()).await.unwrap();
    assert_eq!(a.wait_text().await, "made it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_code_fails_on_both_sides_and_kills_the_code() {
    let rv = rendezvous().await;
    let (mut a, mut b, mut c) = (
        node(&rv.url, "Alpha"),
        node(&rv.url, "Bravo"),
        node(&rv.url, "Charlie"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    let nameplate = code.split('-').next().unwrap();
    let wrong = format!("{nameplate}-aardvark-abandoned-abbreviate");
    assert_ne!(wrong, code);

    b.tunnel.join_code(&wrong, false).await.unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_failed"));
    assert_eq!(a.wait_ended().await.as_deref(), Some("code_failed"));

    // The code is dead: the real code no longer works either.
    c.tunnel.join_code(&code, false).await.unwrap();
    assert_eq!(c.wait_ended().await.as_deref(), Some("code_failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typed_codes_are_checked_before_anything_happens() {
    let rv = rendezvous().await;
    let b = node(&rv.url, "Bravo");
    for bad in [
        "",
        "hello",
        "42",
        "42-aardvark",
        "0-aardvark-abandoned-abbreviate",
        "42-zzzz-abandoned-abbreviate",
    ] {
        let error = b.tunnel.join_code(bad, false).await.unwrap_err();
        assert_eq!(error.code(), "code_invalid", "{bad}");
    }
    assert!(!b.tunnel.has_session());
    // A link naming another pairing service needs the person's say-so.
    let link = "tunnel://pair?c=42-aardvark-abandoned-abbreviate&s=other.example.com";
    assert_eq!(
        b.tunnel.check_code(link).unwrap().other_host.as_deref(),
        Some("other.example.com")
    );
    assert!(b.tunnel.join_code(link, false).await.is_err());
    assert!(!b.tunnel.has_session());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_code_and_an_unreachable_service_are_reported() {
    let rv = rendezvous().await;
    let mut b = node(&rv.url, "Bravo");
    b.tunnel
        .join_code("9876-aardvark-abandoned-abbreviate", false)
        .await
        .unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_failed"));

    let mut offline = node("ws://127.0.0.1:9/v1", "Offline");
    offline.tunnel.create_code().await.unwrap();
    assert_eq!(
        offline.wait_ended().await.as_deref(),
        Some("rendezvous_unreachable")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_code_expires() {
    let config = tunnel_rendezvous::Config {
        room_lifetime: Duration::from_secs(1),
        ..Default::default()
    };
    let rv = rendezvous_with(config).await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    assert_eq!(a.wait_ended().await.as_deref(), Some("code_expired"));
    b.tunnel.join_code(&code, false).await.unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("code_expired"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_device_is_told_the_code_is_in_use_and_the_pair_is_unaffected() {
    let rv = rendezvous().await;
    let (mut a, mut b, mut c) = (
        node(&rv.url, "Alpha"),
        node(&rv.url, "Bravo"),
        node(&rv.url, "Charlie"),
    );
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    b.tunnel.join_code(&code, false).await.unwrap();
    let (safety_a, safety_b) = tokio::join!(a.wait_safety(), b.wait_safety());
    assert_eq!(safety_a, safety_b);
    // Charlie tries the same code afterwards: the code worked once and is dead.
    // (A third device arriving while the room is still open gets
    // `code_in_use`; that path is covered by the rendezvous server's tests.)
    c.tunnel.join_code(&code, false).await.unwrap();
    let outcome = c.wait_ended().await;
    assert!(
        matches!(outcome.as_deref(), Some("code_in_use" | "code_failed")),
        "{outcome:?}"
    );

    a.tunnel.confirm_safety(true).await.unwrap();
    b.tunnel.confirm_safety(true).await.unwrap();
    tokio::join!(a.wait_active(), b.wait_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saying_the_safety_codes_differ_ends_the_session_on_both_sides() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    a.tunnel.create_code().await.unwrap();
    let code = a.wait_code().await;
    b.tunnel.join_code(&code, false).await.unwrap();
    tokio::join!(a.wait_safety(), b.wait_safety());

    // Nothing can be sent before both people confirm.
    assert!(a.tunnel.send_text("too early".into()).await.is_err());
    assert!(
        a.tunnel
            .send_paths(vec![std::env::temp_dir()])
            .await
            .is_err()
    );

    a.tunnel.confirm_safety(true).await.unwrap();
    b.tunnel.confirm_safety(false).await.unwrap();
    assert_eq!(b.wait_ended().await.as_deref(), Some("safety_mismatch"));
    assert_eq!(a.wait_ended().await.as_deref(), Some("safety_mismatch"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_while_showing_a_code_frees_everything() {
    let rv = rendezvous().await;
    let mut a = node(&rv.url, "Alpha");
    a.tunnel.create_code().await.unwrap();
    a.wait_code().await;
    assert!(
        a.tunnel.create_code().await.is_err(),
        "one session at a time"
    );
    a.tunnel.end_session().await;
    assert!(!a.tunnel.has_session());
    assert_eq!(a.wait_ended().await, None, "cancelling is not an error");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rv.server.stats().rooms, 0);
    // A new code can be made straight away.
    a.tunnel.create_code().await.unwrap();
    a.wait_code().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_that_changes_or_vanishes_is_reported_and_the_rest_arrives() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("shifting");
    write_file(&root.join("stable.txt"), b"stable");
    write_file(&root.join("changes.txt"), b"before");
    write_file(&root.join("vanishes.txt"), b"soon gone");

    let id = a.tunnel.send_paths(vec![root.clone()]).await.unwrap();
    b.wait_offer().await;
    // Between the offer and the accept, one file changes and one disappears.
    write_file(&root.join("changes.txt"), b"after, and longer");
    std::fs::remove_file(root.join("vanishes.txt")).unwrap();
    b.tunnel.respond_offer(&id, true, None).await.unwrap();

    let received = b.wait_transfer_end(&id).await;
    assert_eq!(received.state, TransferState::Done);
    assert_eq!(
        (
            received.files_done,
            received.files_failed,
            received.files_skipped
        ),
        (1, 1, 1)
    );
    let reasons: Vec<(&str, &str)> = received
        .issues
        .iter()
        .map(|i| (i.path.as_str(), i.reason))
        .collect();
    assert!(
        reasons.contains(&("shifting/changes.txt", "changed")),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&("shifting/vanishes.txt", "unreadable_on_sender")),
        "{reasons:?}"
    );
    assert_eq!(tree(&b.saved("shifting")), [("stable.txt".to_string(), 6)]);
    let sent = a.wait_transfer_end(&id).await;
    assert_eq!(
        (sent.files_done, sent.files_failed, sent.files_skipped),
        (1, 1, 1)
    );
    assert_eq!(b.tunnel.history(1).unwrap()[0].result, "partial");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_connections_are_reported_as_direct() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;
    // Relays are off in tests, so the path can only be direct.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let SessionState::Active {
            path: PathKind::Direct,
            ..
        } = a.tunnel.snapshot().session
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "path never became direct"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// R2/R4 at full size. Run with `cargo test --release -- --ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "large: run in release mode"]
async fn five_gigabyte_sparse_file() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("sparse.bin");
    let size = 5u64 * 1024 * 1024 * 1024;
    std::fs::File::create(&file).unwrap().set_len(size).unwrap();
    let started = std::time::Instant::now();
    let id = a.tunnel.send_paths(vec![file]).await.unwrap();
    b.wait_offer().await;
    b.tunnel.respond_offer(&id, true, None).await.unwrap();
    // Slow machines need more than the usual step limit for 5 GiB.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1800);
    let received = loop {
        let event = tokio::time::timeout_at(deadline, b.events.recv())
            .await
            .expect("timed out");
        if let Ok(Event::Transfer { transfer }) = event
            && transfer.id == id
            && transfer.state.is_final()
        {
            break transfer;
        }
    };
    assert_eq!(received.state, TransferState::Done);
    assert_eq!(
        std::fs::metadata(b.saved("sparse.bin")).unwrap().len(),
        size
    );
    let secs = started.elapsed().as_secs_f64();
    println!(
        "5 GiB in {secs:.1} s = {:.0} MB/s",
        size as f64 / 1e6 / secs
    );
}

/// R4 at full size. Run with `cargo test --release -- --ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "large: run in release mode"]
async fn one_hundred_thousand_small_files() {
    let rv = rendezvous().await;
    let (mut a, mut b) = (node(&rv.url, "Alpha"), node(&rv.url, "Bravo"));
    pair(&mut a, &mut b).await;
    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("hundred-thousand");
    let body = pattern(5, 10 * 1024);
    for i in 0..100_000 {
        write_file(&root.join(format!("d{:03}/f{i:06}.bin", i % 500)), &body);
    }
    let started = std::time::Instant::now();
    let id = a.tunnel.send_paths(vec![root]).await.unwrap();
    b.wait_offer().await;
    b.tunnel.respond_offer(&id, true, None).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1800);
    let received = loop {
        let event = tokio::time::timeout_at(deadline, b.events.recv())
            .await
            .expect("timed out");
        if let Ok(Event::Transfer { transfer }) = event
            && transfer.id == id
            && transfer.state.is_final()
        {
            break transfer;
        }
    };
    assert_eq!(received.state, TransferState::Done);
    assert_eq!((received.files_done, received.files_failed), (100_000, 0));
    println!("100,000 files in {:.1} s", started.elapsed().as_secs_f64());
}
