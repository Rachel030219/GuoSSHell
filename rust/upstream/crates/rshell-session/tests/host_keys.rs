use std::{fs, time::Duration};

use rshell_core::{HostKeyDecision, InteractionRequest, InteractionResponse, SessionFailure};
use rshell_platform::private_file_is_secure;
use rshell_session::{HostKeyError, KnownHostsVerifier, interaction_channel};
use russh::keys::{PublicKey, parse_public_key_base64};
use tempfile::TempDir;

const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILagOJFgwaMNhBWQINinKOXmqS4Gh5NgxgriXwdOoINJ";
const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";

fn key_a() -> PublicKey {
    parse_public_key_base64(KEY_A).expect("valid ed25519 test key")
}

fn key_b() -> PublicKey {
    parse_public_key_base64(KEY_B).expect("valid distinct ed25519 test key")
}

fn verifier(temp: &TempDir) -> KnownHostsVerifier {
    KnownHostsVerifier::new(temp.path().join("config").join("known_hosts"))
        .with_timeout(Duration::from_millis(50))
}

fn assert_no_known_hosts_temporary_files(verifier: &KnownHostsVerifier) {
    let leftovers = fs::read_dir(verifier.path().parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with(".rshell-known-hosts-"))
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "temporary known-hosts files leaked");
}

async fn accept_unknown(verifier: &KnownHostsVerifier, key: &PublicKey, host: &str, port: u16) {
    let (broker, mut requests) = interaction_channel();
    let verification = verifier.verify(host, port, key, &broker);
    tokio::pin!(verification);
    let request = tokio::select! {
        request = requests.recv() => request.expect("host-key request"),
        result = &mut verification => panic!("unexpected verification result: {result:?}"),
    };
    let InteractionRequest::HostKey(prompt) = request.1 else {
        panic!("expected host-key prompt");
    };
    broker
        .respond(
            prompt.id,
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
        )
        .expect("host-key response accepted");
    verification.await.expect("unknown key accepted and stored");
}

#[tokio::test]
async fn known_key_completes_without_a_prompt() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let key = key_a();
    accept_unknown(&verifier, &key, "known.test", 22).await;

    let (broker, mut requests) = interaction_channel();
    verifier
        .verify("known.test", 22, &key, &broker)
        .await
        .expect("stored key is known");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), requests.recv())
            .await
            .is_err(),
        "known keys must not prompt"
    );
}

#[tokio::test]
async fn unknown_key_requires_algorithm_and_sha256_confirmation_before_learning() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let stored_key = key_a();
    accept_unknown(&verifier, &stored_key, "preserved.test", 22).await;
    let before = fs::read(verifier.path()).unwrap();

    let key = key_b();
    let (broker, mut requests) = interaction_channel();
    let verification = verifier.verify("unknown.test", 2222, &key, &broker);
    tokio::pin!(verification);
    let request = tokio::select! {
        request = requests.recv() => request.expect("host-key request"),
        result = &mut verification => panic!("unexpected verification result: {result:?}"),
    };
    let InteractionRequest::HostKey(prompt) = request.1 else {
        panic!("expected host-key prompt");
    };
    assert_eq!(prompt.host, "unknown.test");
    assert_eq!(prompt.port, 2222);
    assert_eq!(prompt.algorithm, "ssh-ed25519");
    assert!(prompt.sha256.starts_with("SHA256:"));
    assert!(!prompt.changed);
    assert_eq!(fs::read(verifier.path()).unwrap(), before);

    broker
        .respond(
            prompt.id,
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
        )
        .expect("host-key response accepted");
    verification.await.expect("accepted unknown key stored");

    assert!(fs::read(verifier.path()).unwrap().starts_with(&before));
    assert!(private_file_is_secure(verifier.path()).unwrap());
    assert_no_known_hosts_temporary_files(&verifier);
    let (broker, _requests) = interaction_channel();
    verifier
        .verify("unknown.test", 2222, &key, &broker)
        .await
        .expect("accepted key is now known");
}

#[tokio::test]
async fn rejected_unknown_key_leaves_live_file_unchanged() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let existing = key_a();
    accept_unknown(&verifier, &existing, "preserved.test", 22).await;
    let before = fs::read(verifier.path()).unwrap();

    let key = key_b();
    let (broker, mut requests) = interaction_channel();
    let verification = verifier.verify("rejected.test", 22, &key, &broker);
    tokio::pin!(verification);
    let request = tokio::select! {
        request = requests.recv() => request.expect("host-key request"),
        result = &mut verification => panic!("unexpected verification result: {result:?}"),
    };
    let InteractionRequest::HostKey(prompt) = request.1 else {
        panic!("expected host-key prompt");
    };
    broker
        .respond(
            prompt.id,
            InteractionResponse::HostKey(HostKeyDecision::Reject),
        )
        .unwrap();
    assert!(matches!(
        verification.await,
        Err(HostKeyError::Rejected { .. })
    ));
    assert_eq!(fs::read(verifier.path()).unwrap(), before);
}

#[tokio::test(start_paused = true)]
async fn unanswered_unknown_key_times_out_and_cleans_the_pending_response() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let key = key_a();
    let (broker, mut requests) = interaction_channel();
    let verification = verifier.verify("timeout.test", 22, &key, &broker);
    tokio::pin!(verification);
    let request = tokio::select! {
        request = requests.recv() => request.expect("host-key request"),
        result = &mut verification => panic!("unexpected verification result: {result:?}"),
    };
    let InteractionRequest::HostKey(prompt) = request.1 else {
        panic!("expected host-key prompt");
    };
    tokio::time::advance(Duration::from_millis(51)).await;
    assert!(matches!(
        verification.await,
        Err(HostKeyError::Timeout { .. })
    ));
    assert_eq!(
        broker
            .respond(
                prompt.id,
                InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
            )
            .expect_err("expired response must not remain pending")
            .failure(),
        SessionFailure::Validation
    );
    assert!(!verifier.path().exists());
}

#[tokio::test]
async fn changed_key_fails_closed_without_prompt_or_live_mutation() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let original = key_a();
    accept_unknown(&verifier, &original, "changed.test", 22).await;
    let before = fs::read(verifier.path()).unwrap();

    let (broker, mut requests) = interaction_channel();
    let error = verifier
        .verify("changed.test", 22, &key_b(), &broker)
        .await
        .expect_err("changed key must be rejected before interaction");
    assert!(matches!(&error, HostKeyError::Changed { .. }));
    assert_eq!(error.failure(), SessionFailure::HostKeyChanged);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), requests.recv())
            .await
            .is_err(),
        "changed keys must never have an acceptance path"
    );
    assert_eq!(fs::read(verifier.path()).unwrap(), before);
    for output in [format!("{error:?}"), error.to_string()] {
        assert!(!output.contains(KEY_A));
        assert!(!output.contains(KEY_B));
    }
}

#[tokio::test]
async fn zero_port_host_key_decision_is_retained_without_default_port_rewriting() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let key = key_a();
    accept_unknown(&verifier, &key, "zero-port.test", 0).await;

    let contents = fs::read_to_string(verifier.path()).unwrap();
    assert!(contents.contains("[zero-port.test]:0 "));

    let (broker, mut requests) = interaction_channel();
    verifier
        .verify("zero-port.test", 0, &key, &broker)
        .await
        .expect("accepted zero-port decision must remain known");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), requests.recv())
            .await
            .is_err(),
        "retained zero-port decisions must not prompt again"
    );
}

#[tokio::test]
async fn non_file_known_hosts_destination_fails_verification_without_prompt_or_mutation() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    fs::create_dir_all(verifier.path()).unwrap();
    let (broker, mut requests) = interaction_channel();
    let key = key_a();
    assert!(matches!(
        verifier
            .verify("storage-failure.test", 22, &key, &broker)
            .await,
        Err(HostKeyError::Verification { .. })
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), requests.recv())
            .await
            .is_err(),
        "invalid known-hosts storage must fail before prompting"
    );
    assert!(verifier.path().is_dir());
    let entries = fs::read_dir(verifier.path().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, ["known_hosts"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_accepts_for_different_hosts_preserve_both_known_host_entries() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let first_key = key_a();
    let second_key = key_b();
    let (broker_a, mut requests_a) = interaction_channel();
    let (broker_b, mut requests_b) = interaction_channel();
    let verifier_a = verifier.clone();
    let verifier_b = verifier.clone();
    let request_broker_a = broker_a.clone();
    let request_broker_b = broker_b.clone();

    let task_a = tokio::spawn(async move {
        verifier_a
            .verify("first.test", 22, &first_key, &request_broker_a)
            .await
    });
    let task_b = tokio::spawn(async move {
        verifier_b
            .verify("second.test", 22, &second_key, &request_broker_b)
            .await
    });
    let InteractionRequest::HostKey(prompt_a) = requests_a.recv().await.unwrap().1 else {
        panic!("expected first host-key prompt");
    };
    let InteractionRequest::HostKey(prompt_b) = requests_b.recv().await.unwrap().1 else {
        panic!("expected second host-key prompt");
    };
    broker_a
        .respond(
            prompt_a.id,
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
        )
        .unwrap();
    broker_b
        .respond(
            prompt_b.id,
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
        )
        .unwrap();

    task_a.await.unwrap().unwrap();
    task_b.await.unwrap().unwrap();
    let (broker, _requests) = interaction_channel();
    verifier
        .verify("first.test", 22, &key_a(), &broker)
        .await
        .unwrap();
    verifier
        .verify("second.test", 22, &key_b(), &broker)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_accepts_for_the_same_host_trust_exactly_one_key() {
    let temp = TempDir::new().unwrap();
    let verifier = verifier(&temp);
    let first_key = key_a();
    let second_key = key_b();
    let (broker_a, mut requests_a) = interaction_channel();
    let (broker_b, mut requests_b) = interaction_channel();
    let verifier_a = verifier.clone();
    let verifier_b = verifier.clone();
    let request_broker_a = broker_a.clone();
    let request_broker_b = broker_b.clone();

    let task_a = tokio::spawn(async move {
        verifier_a
            .verify("same.test", 22, &first_key, &request_broker_a)
            .await
    });
    let task_b = tokio::spawn(async move {
        verifier_b
            .verify("same.test", 22, &second_key, &request_broker_b)
            .await
    });
    let InteractionRequest::HostKey(prompt_a) = requests_a.recv().await.unwrap().1 else {
        panic!("expected first host-key prompt");
    };
    let InteractionRequest::HostKey(prompt_b) = requests_b.recv().await.unwrap().1 else {
        panic!("expected second host-key prompt");
    };
    broker_a
        .respond(
            prompt_a.id,
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
        )
        .unwrap();
    broker_b
        .respond(
            prompt_b.id,
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
        )
        .unwrap();

    let result_a = task_a.await.unwrap();
    let result_b = task_b.await.unwrap();
    assert!(
        matches!(
            (&result_a, &result_b),
            (Ok(()), Err(HostKeyError::Changed { .. }))
                | (Err(HostKeyError::Changed { .. }), Ok(()))
        ),
        "same host must not accept two distinct keys"
    );
    let (known_key, changed_key) = if result_a.is_ok() {
        (key_a(), key_b())
    } else {
        (key_b(), key_a())
    };
    let (broker, mut requests) = interaction_channel();
    verifier
        .verify("same.test", 22, &known_key, &broker)
        .await
        .unwrap();
    assert!(matches!(
        verifier
            .verify("same.test", 22, &changed_key, &broker)
            .await,
        Err(HostKeyError::Changed { .. })
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), requests.recv())
            .await
            .is_err(),
        "the changed key must not receive a new acceptance prompt"
    );
}
