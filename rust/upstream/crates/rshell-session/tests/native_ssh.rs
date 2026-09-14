mod support;

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rshell_core::{
    AuthenticationKind, ConnectionProfile, CredentialRef, HostKeyDecision, InteractionRequest,
    InteractionResponse, SessionFailure, SessionState, TerminalProfile, TerminalSize,
    TransportKind,
};
use rshell_session::{
    AuthPlan, InteractionBroker, KnownHostsVerifier, NativeFactory, NativeSshTransport,
    SessionCommand, SessionEvent, SessionLaunch, SessionManager, SessionTransport,
    TransportCapabilities, TransportEvent, TransportRequest, interaction_channel,
};
use rshell_storage::{CredentialVault, MemoryCredentialVault};
use secrecy::SecretString;
use tempfile::TempDir;
use tokio::sync::mpsc;

use support::ssh_server::{
    KBI_ANSWERS, KEY_PASSPHRASE, PASSWORD, ServerAuth, TestSshServer, USERNAME,
    password_fingerprint, start_reset_server, write_encrypted_client_key,
};

const CASE_TIMEOUT: Duration = Duration::from_secs(8);

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize {
        cols,
        rows,
        pixel_width: u32::from(cols) * 10,
        pixel_height: u32::from(rows) * 20,
        dpi: 96,
    }
}

fn profile(address: SocketAddr, authentication: AuthenticationKind) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new("native contract", address.ip().to_string());
    profile.port = address.port();
    profile.username = USERNAME.to_owned();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = authentication;
    profile
}

fn vault_plan(mut profile: ConnectionProfile, secret: &str) -> (ConnectionProfile, AuthPlan) {
    let reference = CredentialRef::new("native-contract-secret");
    profile.credential_ref = Some(reference.clone());
    let vault = MemoryCredentialVault::new();
    vault
        .put(&reference, &SecretString::from(secret.to_owned()))
        .expect("store contract credential");
    let plan = AuthPlan::from_profile(&profile, &vault).expect("build native auth plan");
    assert_eq!(vault.call_counts().get, 1);
    (profile, plan)
}

fn transport(
    profile: ConnectionProfile,
    auth: AuthPlan,
    directory: &TempDir,
) -> NativeSshTransport {
    NativeSshTransport::new(
        profile,
        auth,
        rshell_session::KnownHostsVerifier::new(directory.path().join("known_hosts"))
            .with_timeout(Duration::from_secs(2)),
    )
    .expect("valid native transport")
}

async fn drive_connect<F>(
    transport: &mut NativeSshTransport,
    request: &TransportRequest,
    mut respond: F,
) -> (
    Result<(), rshell_session::TransportError>,
    Vec<InteractionRequest>,
)
where
    F: FnMut(&InteractionRequest) -> Option<InteractionResponse>,
{
    let (broker, mut requests) = interaction_channel();
    let response_broker = broker.clone();
    let connect = transport.connect(request, broker);
    tokio::pin!(connect);
    let mut seen = Vec::new();
    let result = tokio::time::timeout(CASE_TIMEOUT, async {
        loop {
            tokio::select! {
                result = &mut connect => return result,
                request = requests.recv() => {
                    let (id, request) = request.expect("interaction channel closed during connect");
                    let response = respond(&request);
                    seen.push(request);
                    if let Some(response) = response {
                        response_broker.respond(id, response).expect("respond to interaction");
                    }
                }
            }
        }
    })
    .await
    .expect("native SSH connect hung");
    (result, seen)
}

async fn connect_accepting_host(
    transport: &mut NativeSshTransport,
    request: &TransportRequest,
) -> (
    Result<(), rshell_session::TransportError>,
    Vec<InteractionRequest>,
) {
    drive_connect(transport, request, |request| match request {
        InteractionRequest::HostKey(_) => Some(InteractionResponse::HostKey(
            HostKeyDecision::AcceptAndStore,
        )),
        _ => panic!("unexpected authentication interaction: {request:?}"),
    })
    .await
}

async fn output_until(transport: &mut NativeSshTransport, marker: &[u8]) -> Vec<u8> {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("native SSH event") {
                TransportEvent::Output(bytes) => {
                    output.extend_from_slice(&bytes);
                    if contains(&output, marker) {
                        return output;
                    }
                }
                event => panic!("unexpected native SSH event before marker: {event:?}"),
            }
        }
    })
    .await
    .expect("native SSH output timed out")
}

#[tokio::test]
async fn password_auth_confirms_unknown_key_then_echoes_and_resizes_pty() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let temp = TempDir::new().unwrap();
    let (profile, auth) = vault_plan(
        profile(server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    let mut transport = transport(profile, auth, &temp);
    let request = TransportRequest::new(size(80, 24));

    assert_eq!(
        transport.capabilities(),
        TransportCapabilities {
            managed_password: true,
            public_key: true,
            agent: false,
            keyboard_interactive: true,
            host_key_prompt: true,
        }
    );
    let (result, prompts) = connect_accepting_host(&mut transport, &request).await;
    result.expect("password authentication");
    assert_eq!(
        prompts
            .iter()
            .filter(|request| matches!(request, InteractionRequest::HostKey(_)))
            .count(),
        1
    );
    let startup = output_until(&mut transport, b"READY").await;
    assert!(contains(&startup, b"READY"));
    transport
        .write(b"password-ok\r\n")
        .await
        .expect("write native SSH input");
    let echoed = output_until(&mut transport, b"password-ok\r\n").await;
    assert!(contains(&echoed, b"password-ok\r\n"));
    transport
        .resize(size(132, 43))
        .await
        .expect("resize native SSH PTY");
    let resized = output_until(&mut transport, b"RESIZED:132x43").await;
    assert!(contains(&resized, b"RESIZED:132x43"));
    assert_eq!(
        server.snapshot().initial_pty,
        Some(("xterm-256color".to_owned(), 80, 24, 800, 480))
    );
    assert_eq!(server.snapshot().last_size, Some((132, 43, 1320, 860)));
    transport.shutdown().await.expect("first native shutdown");
    transport
        .shutdown()
        .await
        .expect("idempotent native shutdown");
    let snapshot = server.shutdown().await;
    assert_eq!(snapshot.successful_authentications, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_reconnect_exhausts_auth_before_second_network_attempt() {
    let credential = SecretString::from(format!(
        "native-reconnect-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos()
    ));
    let server = TestSshServer::start(ServerAuth::PasswordFingerprint(password_fingerprint(
        &credential,
    )))
    .await;
    let directory = TempDir::new().unwrap();
    let connection = profile(server.address(), AuthenticationKind::Password);
    let auth = AuthPlan::from_secret(&connection, Some(credential)).unwrap();
    let factory = Arc::new(NativeFactory::new(
        connection,
        auth,
        KnownHostsVerifier::new(directory.path().join("known_hosts"))
            .with_timeout(Duration::from_secs(2)),
    ));
    let manager = SessionManager::new(factory);
    let terminal = TerminalProfile::default()
        .settings
        .resolve(&Default::default());
    let launch =
        SessionLaunch::with_default_engine(TransportRequest::new(size(80, 24)), &terminal).unwrap();
    let mut client = manager.launch(launch).unwrap();

    tokio::time::timeout(CASE_TIMEOUT, async {
        loop {
            match client.events.recv().await.unwrap() {
                SessionEvent::InteractionRequired(InteractionRequest::HostKey(prompt)) => client
                    .try_command(SessionCommand::Respond(
                        prompt.id,
                        InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
                    ))
                    .unwrap(),
                SessionEvent::StateChanged(SessionState::Connected) => break,
                SessionEvent::Failed(failure) => {
                    panic!("initial native connection failed: {failure:?}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("initial native connection timed out");
    assert_eq!(server.snapshot().accepted_connections, 1);

    client.try_command(SessionCommand::Reconnect).unwrap();
    let failure = tokio::time::timeout(CASE_TIMEOUT, async {
        loop {
            if let SessionEvent::Failed(failure) = client.events.recv().await.unwrap() {
                break failure;
            }
        }
    })
    .await
    .expect("actor reconnect did not fail");

    assert_eq!(failure, SessionFailure::Authentication);
    assert_eq!(server.snapshot().accepted_connections, 1);
    manager.shutdown_all().await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn localhost_alias_uses_the_connected_peer_for_host_key_lookup() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let temp = TempDir::new().unwrap();
    let verifier = rshell_session::KnownHostsVerifier::new(temp.path().join("known_hosts"))
        .with_timeout(Duration::from_secs(2));

    let mut alias_profile = profile(server.address(), AuthenticationKind::Password);
    alias_profile.host = "localhost".to_owned();
    let (alias_profile, alias_auth) = vault_plan(alias_profile, PASSWORD);
    let mut alias_transport =
        NativeSshTransport::new(alias_profile, alias_auth, verifier.clone()).unwrap();
    let request = TransportRequest::new(size(80, 24));
    let (result, prompts) = connect_accepting_host(&mut alias_transport, &request).await;
    result.expect("localhost alias authentication");
    let InteractionRequest::HostKey(prompt) = prompts.first().expect("peer host-key prompt") else {
        panic!("expected host-key prompt");
    };
    assert_eq!(prompt.host, server.address().ip().to_string());
    assert_eq!(prompt.port, server.address().port());
    alias_transport.shutdown().await.unwrap();

    let (ip_profile, ip_auth) = vault_plan(
        profile(server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    let mut ip_transport = NativeSshTransport::new(ip_profile, ip_auth, verifier).unwrap();
    let (result, prompts) = drive_connect(&mut ip_transport, &request, |request| {
        panic!("peer identity should already be known: {request:?}")
    })
    .await;
    result.expect("peer IP authentication after alias acceptance");
    assert!(prompts.is_empty());
    ip_transport.shutdown().await.unwrap();

    let snapshot = server.shutdown().await;
    assert_eq!(snapshot.successful_authentications, 2);
}

#[tokio::test]
async fn rejected_host_key_does_not_consume_password_authentication() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let temp = TempDir::new().unwrap();
    let (profile, auth) = vault_plan(
        profile(server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    let mut transport = transport(profile, auth, &temp);
    let request = TransportRequest::new(size(80, 24));

    let (rejected, _) = drive_connect(&mut transport, &request, |request| match request {
        InteractionRequest::HostKey(_) => {
            Some(InteractionResponse::HostKey(HostKeyDecision::Reject))
        }
        _ => panic!("unexpected authentication interaction: {request:?}"),
    })
    .await;
    assert_eq!(
        rejected.unwrap_err().failure(),
        SessionFailure::HostKeyRejected
    );

    let (accepted, _) = connect_accepting_host(&mut transport, &request).await;
    accepted.expect("password authentication must survive host-key rejection");
    transport.shutdown().await.unwrap();

    let snapshot = server.shutdown().await;
    assert_eq!(snapshot.successful_authentications, 1);
}

#[tokio::test]
async fn encrypted_private_key_uses_vault_passphrase_and_reuses_known_host() {
    let temp = TempDir::new().unwrap();
    let (key_path, public_key) = write_encrypted_client_key(temp.path());
    let server = TestSshServer::start(ServerAuth::PublicKey(public_key)).await;
    let mut prompt_count = 0;

    for _ in 0..2 {
        let mut connection = profile(server.address(), AuthenticationKind::PublicKey);
        connection.identity_file = Some(key_path.clone());
        let (connection, auth) = vault_plan(connection, KEY_PASSPHRASE);
        let mut transport = transport(connection, auth, &temp);
        let request = TransportRequest::new(size(90, 30));
        let (result, prompts) = connect_accepting_host(&mut transport, &request).await;
        result.expect("encrypted public-key authentication");
        prompt_count += prompts
            .iter()
            .filter(|request| matches!(request, InteractionRequest::HostKey(_)))
            .count();
        transport.shutdown().await.unwrap();
    }

    assert_eq!(prompt_count, 1, "known host must be reused");
    let snapshot = server.shutdown().await;
    assert_eq!(snapshot.successful_authentications, 2);
}

#[tokio::test]
async fn keyboard_interactive_round_trips_multiple_echo_flags() {
    let server = TestSshServer::start(ServerAuth::KeyboardInteractive).await;
    let temp = TempDir::new().unwrap();
    let connection = profile(server.address(), AuthenticationKind::KeyboardInteractive);
    let auth = AuthPlan::from_profile(&connection, &MemoryCredentialVault::new()).unwrap();
    let mut transport = transport(connection, auth, &temp);
    let request = TransportRequest::new(size(80, 24));
    let (result, prompts) = drive_connect(&mut transport, &request, |request| match request {
        InteractionRequest::HostKey(_) => Some(InteractionResponse::HostKey(
            HostKeyDecision::AcceptAndStore,
        )),
        InteractionRequest::KeyboardInteractive(prompt) => {
            assert_eq!(prompt.name, "Contract authentication");
            assert_eq!(prompt.instruction, "Supply both answers in order");
            assert_eq!(
                prompt
                    .prompts
                    .iter()
                    .map(|prompt| (prompt.label.as_str(), prompt.echo))
                    .collect::<Vec<_>>(),
                [("Visible answer", true), ("One-time code", false)]
            );
            Some(InteractionResponse::Answers(
                KBI_ANSWERS
                    .iter()
                    .map(|answer| SecretString::from((*answer).to_owned()))
                    .collect(),
            ))
        }
        request => panic!("unexpected interaction: {request:?}"),
    })
    .await;
    result.expect("keyboard-interactive authentication");
    assert_eq!(
        prompts
            .iter()
            .filter(|request| matches!(request, InteractionRequest::KeyboardInteractive(_)))
            .count(),
        1
    );
    assert_eq!(server.snapshot().keyboard_answers, KBI_ANSWERS);
    transport.shutdown().await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn wrong_password_changed_key_and_reset_map_to_distinct_failures() {
    let request = TransportRequest::new(size(80, 24));

    let wrong_server = TestSshServer::start(ServerAuth::Password).await;
    let wrong_temp = TempDir::new().unwrap();
    let (connection, auth) = vault_plan(
        profile(wrong_server.address(), AuthenticationKind::Password),
        "incorrect-password",
    );
    let mut wrong = transport(connection, auth, &wrong_temp);
    let (result, _) = connect_accepting_host(&mut wrong, &request).await;
    assert_eq!(
        result.unwrap_err().failure(),
        SessionFailure::Authentication
    );
    let accepted_connections = wrong_server.snapshot().accepted_connections;
    let (broker, mut requests) = interaction_channel();
    assert_eq!(
        wrong
            .connect(&request, broker)
            .await
            .expect_err("consumed password plan must not be reusable")
            .failure(),
        SessionFailure::Authentication
    );
    assert_eq!(
        wrong_server.snapshot().accepted_connections,
        accepted_connections,
        "a consumed auth plan must fail before opening another connection"
    );
    assert!(
        requests.try_recv().is_err(),
        "a consumed auth plan must fail before host-key interaction"
    );
    wrong.shutdown().await.unwrap();
    let address = wrong_server.address();
    wrong_server.shutdown().await;

    let trusted_server = TestSshServer::start_at(address, ServerAuth::Password).await;
    let changed_temp = TempDir::new().unwrap();
    let (connection, auth) = vault_plan(
        profile(trusted_server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    let mut trusted = transport(connection, auth, &changed_temp);
    connect_accepting_host(&mut trusted, &request)
        .await
        .0
        .unwrap();
    trusted.shutdown().await.unwrap();
    let address = trusted_server.address();
    trusted_server.shutdown().await;

    let changed_server = TestSshServer::start_at(address, ServerAuth::Password).await;
    let (connection, auth) = vault_plan(
        profile(changed_server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    let mut changed = transport(connection, auth, &changed_temp);
    let (result, prompts) = connect_accepting_host(&mut changed, &request).await;
    assert_eq!(
        result.unwrap_err().failure(),
        SessionFailure::HostKeyChanged
    );
    assert!(
        prompts.is_empty(),
        "changed key must have no acceptance prompt"
    );
    changed.shutdown().await.unwrap();
    changed_server.shutdown().await;

    let (reset_address, reset_task) = start_reset_server().await;
    let reset_temp = TempDir::new().unwrap();
    let (connection, auth) = vault_plan(
        profile(reset_address, AuthenticationKind::Password),
        PASSWORD,
    );
    let mut reset = transport(connection, auth, &reset_temp);
    let (broker, _requests) = interaction_channel();
    let error = tokio::time::timeout(CASE_TIMEOUT, reset.connect(&request, broker))
        .await
        .expect("reset connect hung")
        .unwrap_err();
    assert_eq!(error.failure(), SessionFailure::Network);
    reset.shutdown().await.unwrap();
    reset_task.await.unwrap();
}

#[tokio::test]
async fn remote_command_preserves_output_nonzero_exit_and_eof() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let temp = TempDir::new().unwrap();
    let mut connection = profile(server.address(), AuthenticationKind::Password);
    connection.remote_command = Some("exit:37".to_owned());
    let (connection, auth) = vault_plan(connection, PASSWORD);
    let mut transport = transport(connection, auth, &temp);
    connect_accepting_host(&mut transport, &TransportRequest::new(size(80, 24)))
        .await
        .0
        .unwrap();

    let (output, status, eof) = tokio::time::timeout(CASE_TIMEOUT, async {
        let mut output = Vec::new();
        let mut status = None;
        let mut eof = false;
        while status.is_none() || !eof {
            match transport.next_event().await.unwrap() {
                TransportEvent::Output(bytes) => output.extend_from_slice(&bytes),
                TransportEvent::Exit(exit) => status = Some(exit),
                TransportEvent::Eof => eof = true,
                event => panic!("unexpected remote-command event: {event:?}"),
            }
        }
        (output, status.unwrap(), eof)
    })
    .await
    .expect("remote command events hung");
    assert!(contains(&output, b"remote-output-before-exit\r\n"));
    assert_eq!(status.code, Some(37));
    assert!(!status.success);
    assert!(eof);
    assert_eq!(server.snapshot().remote_commands, [b"exit:37".to_vec()]);
    transport.shutdown().await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn keyboard_interactive_cancel_wrong_count_and_timeout_fail_closed() {
    for response in [
        InteractionResponse::Cancel,
        InteractionResponse::Answers(vec![SecretString::from("only-one".to_owned())]),
    ] {
        let server = TestSshServer::start(ServerAuth::KeyboardInteractive).await;
        let temp = TempDir::new().unwrap();
        let connection = profile(server.address(), AuthenticationKind::KeyboardInteractive);
        let auth = AuthPlan::from_profile(&connection, &MemoryCredentialVault::new()).unwrap();
        let mut transport = transport(connection, auth, &temp);
        let mut response = Some(response);
        let (result, _) = drive_connect(
            &mut transport,
            &TransportRequest::new(size(80, 24)),
            |request| match request {
                InteractionRequest::HostKey(_) => Some(InteractionResponse::HostKey(
                    HostKeyDecision::AcceptAndStore,
                )),
                InteractionRequest::KeyboardInteractive(_) => response.take(),
                request => panic!("unexpected interaction: {request:?}"),
            },
        )
        .await;
        assert_eq!(
            result.unwrap_err().failure(),
            SessionFailure::Authentication
        );
        transport.shutdown().await.unwrap();
        server.shutdown().await;
    }

    let server = TestSshServer::start(ServerAuth::KeyboardInteractive).await;
    let temp = TempDir::new().unwrap();
    let connection = profile(server.address(), AuthenticationKind::KeyboardInteractive);
    let auth = AuthPlan::from_profile(&connection, &MemoryCredentialVault::new()).unwrap();
    let mut transport = transport(connection, auth, &temp)
        .with_timeout(Duration::from_secs(30))
        .expect("nonzero native timeout");
    let (broker, mut requests): (InteractionBroker, mpsc::Receiver<_>) = interaction_channel();
    let response_broker = broker.clone();
    let request = TransportRequest::new(size(80, 24));
    let mut pending_keyboard_id = None;
    let mut clock_paused = false;
    let result = {
        let connect = transport.connect(&request, broker);
        tokio::pin!(connect);
        loop {
            tokio::select! {
                result = &mut connect => break result,
                request = requests.recv() => {
                    let (id, request) = request.unwrap();
                    match request {
                        InteractionRequest::HostKey(_) => response_broker.respond(
                            id,
                            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
                        ).unwrap(),
                        InteractionRequest::KeyboardInteractive(_) => {
                            pending_keyboard_id = Some(id);
                            tokio::time::pause();
                            clock_paused = true;
                            tokio::time::advance(Duration::from_secs(31)).await;
                        }
                        request => panic!("unexpected interaction: {request:?}"),
                    }
                }
            }
        }
    };
    if clock_paused {
        tokio::time::resume();
    }
    assert_eq!(result.unwrap_err().failure(), SessionFailure::Timeout);
    assert_eq!(
        response_broker
            .respond(
                pending_keyboard_id.expect("keyboard prompt before timeout"),
                InteractionResponse::Cancel,
            )
            .unwrap_err()
            .failure(),
        SessionFailure::Validation
    );
    transport.shutdown().await.unwrap();
    server.shutdown().await;
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
