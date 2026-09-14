mod support;

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rshell_core::{
    AuthenticationKind, ConnectionProfile, CredentialRef, ExitStatus, HostKeyDecision,
    InteractionRequest, InteractionResponse, SessionFailure, TerminalSize, TransportKind,
};
use rshell_session::{
    AuthPlan, InteractionBroker, NativeSshTransport, SessionTransport, SystemOpenSshTransport,
    TransportError, TransportEvent, TransportRequest, interaction_channel,
};
use rshell_storage::{CredentialVault, MemoryCredentialVault};
use russh::{
    ChannelMsg, client,
    keys::{PublicKey, parse_public_key_base64},
};
use secrecy::SecretString;
use tempfile::TempDir;

use support::ssh_server::{
    KBI_ANSWERS, KEY_PASSPHRASE, PASSWORD, REMOTE_COMMAND_OUTPUT, ServerAuth, ServerSnapshot,
    TestSshServer, USERNAME, write_encrypted_client_key,
};

const CASE_TIMEOUT: Duration = Duration::from_secs(8);
const SYSTEM_OPENSSH_TIMEOUT: Duration = Duration::from_secs(30);
const BACKPRESSURE_BYTES: usize = 10 * 1024;
const BACKPRESSURE_MARKER: &[u8] = b"\r\nSMOKE_BURST_COMPLETE\r\n";
const WRONG_PASSWORD: &str = "incorrect-password";
const FIXTURE_WAIT_TIMEOUT: Duration = Duration::from_secs(300);
const FIXTURE_POLL_INTERVAL: Duration = Duration::from_millis(50);
static TEMP_FILE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

const NATIVE_PASSWORD_OBSERVATION_PATH_ENV: &str = "RSHELL_QA_OBSERVATION_NATIVE_PASSWORD_PATH";
const NATIVE_KEY_OBSERVATION_PATH_ENV: &str = "RSHELL_QA_OBSERVATION_NATIVE_KEY_PATH";
const NATIVE_KEYBOARD_INTERACTIVE_OBSERVATION_PATH_ENV: &str =
    "RSHELL_QA_OBSERVATION_NATIVE_KEYBOARD_INTERACTIVE_PATH";
const SYSTEM_AGENT_OBSERVATION_PATH_ENV: &str = "RSHELL_QA_OBSERVATION_SYSTEM_AGENT_PATH";
const SYSTEM_AGENT_PUBLIC_KEY_PATH_ENV: &str = "RSHELL_QA_SYSTEM_AGENT_PUBLIC_KEY_PATH";
const HOST_KEY_OBSERVATION_PATH_ENV: &str = "RSHELL_QA_OBSERVATION_HOST_KEY_PATH";

const FIXTURE_READY_PATH_ENV: &str = "RSHELL_QA_SSH_SMOKE_READY_PATH";
const FIXTURE_STOP_PATH_ENV: &str = "RSHELL_QA_SSH_SMOKE_STOP_PATH";
const FIXTURE_OBSERVATION_DIR_ENV: &str = "RSHELL_QA_SSH_SMOKE_OBSERVATION_DIR";
const FIXTURE_PASSWORD_ENV_NAME_ENV: &str = "RSHELL_QA_SSH_SMOKE_PASSWORD_ENV";
const FIXTURE_ENCRYPTED_KEY_PATH_ENV: &str = "RSHELL_QA_SSH_SMOKE_ENCRYPTED_KEY_PATH";
const FIXTURE_KEY_PASSPHRASE_ENV_NAME_ENV: &str = "RSHELL_QA_SSH_SMOKE_KEY_PASSPHRASE_ENV";
const FIXTURE_KBI_VISIBLE_ANSWER_ENV_NAME_ENV: &str = "RSHELL_QA_SSH_SMOKE_KBI_VISIBLE_ANSWER_ENV";
const FIXTURE_KBI_ONE_TIME_CODE_ENV_NAME_ENV: &str = "RSHELL_QA_SSH_SMOKE_KBI_ONE_TIME_CODE_ENV";
const FIXTURE_EXPECTED_SURFACES_ENV: &str = "RSHELL_QA_SSH_SMOKE_EXPECTED_SURFACES";
const FIXTURE_RUN_NONCE_ENV: &str = "RSHELL_QA_SSH_SMOKE_RUN_NONCE";
const FIXTURE_ID_ENV: &str = "RSHELL_QA_SSH_SMOKE_FIXTURE_ID";
const FIXTURE_AGENT_PUBLIC_KEY_PATH_ENV: &str = "RSHELL_QA_SSH_SMOKE_AGENT_PUBLIC_KEY_PATH";

#[derive(Clone, Copy, PartialEq, Eq)]
enum QaSurface {
    NativePassword,
    NativeKey,
    NativeKeyboardInteractive,
    SystemAgent,
    HostKey,
}

impl QaSurface {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NativePassword => "native_password",
            Self::NativeKey => "native_key",
            Self::NativeKeyboardInteractive => "native_keyboard_interactive",
            Self::SystemAgent => "system_agent",
            Self::HostKey => "host_key",
        }
    }

    const fn observation_path_env(self) -> &'static str {
        match self {
            Self::NativePassword => NATIVE_PASSWORD_OBSERVATION_PATH_ENV,
            Self::NativeKey => NATIVE_KEY_OBSERVATION_PATH_ENV,
            Self::NativeKeyboardInteractive => NATIVE_KEYBOARD_INTERACTIVE_OBSERVATION_PATH_ENV,
            Self::SystemAgent => SYSTEM_AGENT_OBSERVATION_PATH_ENV,
            Self::HostKey => HOST_KEY_OBSERVATION_PATH_ENV,
        }
    }
}

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize {
        cols,
        rows,
        pixel_width: u32::from(cols) * 10,
        pixel_height: u32::from(rows) * 20,
        dpi: 96,
    }
}

fn native_profile(address: SocketAddr, authentication: AuthenticationKind) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new("SSH smoke native", address.ip().to_string());
    profile.port = address.port();
    profile.username = USERNAME.to_owned();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = authentication;
    profile
}

fn system_profile(address: SocketAddr) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new("SSH smoke system agent", address.ip().to_string());
    profile.port = address.port();
    profile.username = USERNAME.to_owned();
    profile.transport = TransportKind::SystemOpenSsh;
    profile.authentication = AuthenticationKind::Agent;
    profile
}

fn vault_plan(mut profile: ConnectionProfile, secret: &str) -> (ConnectionProfile, AuthPlan) {
    let reference = CredentialRef::new("ssh-smoke-secret");
    profile.credential_ref = Some(reference.clone());
    let vault = MemoryCredentialVault::new();
    vault
        .put(&reference, &SecretString::from(secret.to_owned()))
        .expect("store smoke credential");
    let plan = AuthPlan::from_profile(&profile, &vault).expect("build smoke auth plan");
    assert_eq!(vault.call_counts().get, 1);
    (profile, plan)
}

fn native_transport(
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
    .expect("valid native smoke transport")
}

async fn drive_native_connect<F>(
    transport: &mut NativeSshTransport,
    request: &TransportRequest,
    mut respond: F,
) -> (Result<(), TransportError>, Vec<InteractionRequest>)
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
                    let (id, request) = request.expect("interaction channel closed during SSH connect");
                    let response = respond(&request);
                    seen.push(request);
                    if let Some(response) = response {
                        response_broker.respond(id, response).expect("respond to SSH interaction");
                    }
                }
            }
        }
    })
    .await
    .expect("native SSH connect timed out");
    (result, seen)
}

async fn connect_accepting_host(
    transport: &mut NativeSshTransport,
    request: &TransportRequest,
) -> (Result<(), TransportError>, Vec<InteractionRequest>) {
    drive_native_connect(transport, request, |interaction| match interaction {
        InteractionRequest::HostKey(_) => Some(InteractionResponse::HostKey(
            HostKeyDecision::AcceptAndStore,
        )),
        _ => panic!("unexpected authentication interaction"),
    })
    .await
}

async fn output_until<T: SessionTransport>(transport: &mut T, marker: &[u8]) -> Vec<u8> {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("SSH transport event") {
                TransportEvent::Output(bytes) => {
                    output.extend_from_slice(&bytes);
                    if contains(&output, marker) {
                        return output;
                    }
                }
                _ => panic!("SSH transport ended before its expected output"),
            }
        }
    })
    .await
    .expect("SSH output timed out")
}

async fn remote_command_events(transport: &mut NativeSshTransport) -> (Vec<u8>, ExitStatus, bool) {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let mut output = Vec::new();
        let mut status = None;
        let mut eof = false;
        while status.is_none() || !eof {
            match transport.next_event().await.expect("remote-command event") {
                TransportEvent::Output(bytes) => output.extend_from_slice(&bytes),
                TransportEvent::Exit(exit) => status = Some(exit),
                TransportEvent::Eof => eof = true,
                _ => panic!("unexpected remote-command event"),
            }
        }
        (output, status.expect("remote command exit status"), eof)
    })
    .await
    .expect("remote command did not reach EOF")
}

async fn shutdown_native(
    transport: &mut NativeSshTransport,
    server: TestSshServer,
) -> ServerSnapshot {
    transport
        .shutdown()
        .await
        .expect("native SSH shutdown must succeed");
    transport
        .shutdown()
        .await
        .expect("native SSH shutdown must be idempotent");
    shutdown_server(server).await
}

async fn shutdown_server(server: TestSshServer) -> ServerSnapshot {
    let snapshot = server.shutdown().await;
    assert_eq!(snapshot.active_sessions, 0, "server session cleanup");
    assert_eq!(snapshot.open_channels, 0, "server channel cleanup");
    snapshot
}

fn assert_authenticated_channel(snapshot: &ServerSnapshot, surface: QaSurface) {
    assert_eq!(
        snapshot.successful_authentications,
        1,
        "{} server authentication counter",
        surface.as_str()
    );
    assert_eq!(
        snapshot.opened_channels,
        1,
        "{} server channel counter",
        surface.as_str()
    );
    assert_eq!(
        snapshot.active_sessions,
        0,
        "{} server session cleanup",
        surface.as_str()
    );
    assert_eq!(
        snapshot.open_channels,
        0,
        "{} server channel cleanup",
        surface.as_str()
    );
}

fn emit_observation_from_snapshot(
    surface: QaSurface,
    endpoint: SocketAddr,
    snapshot: &ServerSnapshot,
) {
    let Some(path) = std::env::var_os(surface.observation_path_env()) else {
        return;
    };
    let observations = match surface {
        QaSurface::NativePassword => {
            assert_authenticated_channel(snapshot, surface);
            assert_eq!(
                snapshot.password_authentications, 1,
                "native password authentication method"
            );
            ["server_authentication", "server_channel"].as_slice()
        }
        QaSurface::NativeKey => {
            assert_authenticated_channel(snapshot, surface);
            assert_eq!(
                snapshot.public_key_authentications, 1,
                "native key authentication method"
            );
            ["server_authentication", "server_channel"].as_slice()
        }
        QaSurface::NativeKeyboardInteractive => {
            assert_authenticated_channel(snapshot, surface);
            assert_eq!(
                snapshot.keyboard_interactive_authentications, 1,
                "native keyboard-interactive authentication method"
            );
            assert_keyboard_answers(snapshot);
            ["server_authentication", "server_channel"].as_slice()
        }
        QaSurface::SystemAgent => {
            assert_authenticated_channel(snapshot, surface);
            assert_eq!(
                snapshot.public_key_authentications, 1,
                "system agent authentication method"
            );
            ["server_authentication", "server_channel"].as_slice()
        }
        QaSurface::HostKey => {
            unreachable!("host-key observation requires the asserted prompt count")
        }
    };
    write_observation_document(
        Path::new(&path),
        surface,
        observations,
        ObservationBinding::standalone(surface, endpoint),
    );
}

fn emit_host_key_observation(endpoint: SocketAddr, snapshot: &ServerSnapshot, prompt_count: usize) {
    let Some(path) = std::env::var_os(HOST_KEY_OBSERVATION_PATH_ENV) else {
        return;
    };
    assert_eq!(prompt_count, 1, "host key prompt count");
    assert_eq!(
        snapshot.successful_authentications, 0,
        "host key rejection must precede authentication"
    );
    assert_eq!(
        snapshot.opened_channels, 0,
        "host key rejection must precede channel open"
    );
    assert_eq!(
        snapshot.active_sessions, 0,
        "host key server session cleanup"
    );
    assert_eq!(snapshot.open_channels, 0, "host key server channel cleanup");
    write_observation_document(
        Path::new(&path),
        QaSurface::HostKey,
        &["server_host_key_prompt"],
        ObservationBinding::standalone(QaSurface::HostKey, endpoint),
    );
}

struct ObservationBinding<'a> {
    run_nonce: &'a str,
    fixture: &'a str,
    connection: &'a str,
    endpoint: String,
}

impl ObservationBinding<'static> {
    fn standalone(surface: QaSurface, endpoint: SocketAddr) -> Self {
        Self {
            run_nonce: "standalone",
            fixture: "standalone",
            connection: surface.as_str(),
            endpoint: endpoint.to_string(),
        }
    }
}

fn write_observation_document(
    path: &Path,
    surface: QaSurface,
    observations: &[&str],
    binding: ObservationBinding<'_>,
) {
    let parent = path
        .parent()
        .expect("QA observation path must have a parent directory");
    assert!(
        parent.is_dir(),
        "QA observation directory must exist: {}",
        parent.display()
    );
    assert!(
        !path.exists(),
        "QA observation path must be unused: {}",
        path.display()
    );
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .expect("QA observation file name must be UTF-8"),
        std::process::id(),
        sequence
    ));
    let document = format!(
        concat!(
            "{{\"version\":1,\"generated_by\":\"p0_qa\",\"surface\":\"{}\",",
            "\"run_nonce\":\"{}\",\"fixture\":\"{}\",\"connection\":\"{}\",",
            "\"endpoint\":\"{}\",\"observations\":[{}]}}\n"
        ),
        surface.as_str(),
        binding.run_nonce,
        binding.fixture,
        binding.connection,
        binding.endpoint,
        observations
            .iter()
            .map(|observation| format!("\"{observation}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(document.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        panic!(
            "failed to atomically write {} QA observation to {}: {error}",
            surface.as_str(),
            path.display()
        );
    }
}

fn assert_secret_is_redacted(value: &impl std::fmt::Debug, secret: &str) {
    assert!(
        !format!("{value:?}").contains(secret),
        "secret leaked through debug output"
    );
}

fn assert_keyboard_answers(snapshot: &ServerSnapshot) {
    assert_eq!(snapshot.keyboard_answers.len(), KBI_ANSWERS.len());
    assert!(
        snapshot
            .keyboard_answers
            .iter()
            .map(String::as_str)
            .zip(KBI_ANSWERS)
            .all(|(actual, expected)| actual == expected),
        "keyboard-interactive answers were not preserved"
    );
}

#[test]
fn server_snapshot_debug_redacts_captured_authentication_data() {
    let snapshot = ServerSnapshot {
        keyboard_answers: vec!["keyboard-secret".to_owned()],
        received_input: b"terminal-secret".to_vec(),
        remote_commands: vec![b"command-secret".to_vec()],
        ..ServerSnapshot::default()
    };
    let output = format!("{snapshot:?}");
    for secret in ["keyboard-secret", "terminal-secret", "command-secret"] {
        assert!(
            !output.contains(secret),
            "server snapshot debug output leaked a secret"
        );
    }
    assert!(output.contains("[REDACTED]"));
}

#[test]
fn fixture_wait_budget_outlives_the_parent_gtk_smoke_budget() {
    assert!(FIXTURE_WAIT_TIMEOUT > Duration::from_secs(240));
}

#[tokio::test]
async fn native_password_accepts_host_key_and_round_trips_pty() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let endpoint = server.address();
    let temp = TempDir::new().expect("native SSH temp directory");
    let (profile, auth) = vault_plan(
        native_profile(server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    assert_secret_is_redacted(&auth, PASSWORD);
    let mut transport = native_transport(profile, auth, &temp);
    let request = TransportRequest::new(size(80, 24));

    let (result, prompts) = connect_accepting_host(&mut transport, &request).await;
    result.expect("native password authentication");
    assert_eq!(
        prompts
            .iter()
            .filter(|request| matches!(request, InteractionRequest::HostKey(_)))
            .count(),
        1,
        "unknown host must require one confirmation"
    );
    assert!(contains(
        &output_until(&mut transport, b"READY").await,
        b"READY"
    ));

    transport
        .write(b"native-password-ok\r\n")
        .await
        .expect("write native password input");
    assert!(contains(
        &output_until(&mut transport, b"native-password-ok\r\n").await,
        b"native-password-ok\r\n"
    ));
    transport
        .resize(size(132, 43))
        .await
        .expect("resize native password PTY");
    assert!(contains(
        &output_until(&mut transport, b"RESIZED:132x43").await,
        b"RESIZED:132x43"
    ));
    assert_eq!(
        server.snapshot().initial_pty,
        Some(("xterm-256color".to_owned(), 80, 24, 800, 480))
    );
    assert_eq!(server.snapshot().last_size, Some((132, 43, 1320, 860)));

    let snapshot = shutdown_native(&mut transport, server).await;
    assert_eq!(snapshot.successful_authentications, 1);
    emit_observation_from_snapshot(QaSurface::NativePassword, endpoint, &snapshot);
}

#[tokio::test]
async fn native_encrypted_key_uses_passphrase() {
    let temp = TempDir::new().expect("native SSH temp directory");
    let (key_path, public_key) = write_encrypted_client_key(temp.path());
    let server = TestSshServer::start(ServerAuth::PublicKey(public_key)).await;
    let endpoint = server.address();
    let mut profile = native_profile(server.address(), AuthenticationKind::PublicKey);
    profile.identity_file = Some(key_path);
    let (profile, auth) = vault_plan(profile, KEY_PASSPHRASE);
    assert_secret_is_redacted(&auth, KEY_PASSPHRASE);
    let mut transport = native_transport(profile, auth, &temp);

    connect_accepting_host(&mut transport, &TransportRequest::new(size(80, 24)))
        .await
        .0
        .expect("encrypted client key authentication");

    let snapshot = shutdown_native(&mut transport, server).await;
    assert_eq!(snapshot.successful_authentications, 1);
    emit_observation_from_snapshot(QaSurface::NativeKey, endpoint, &snapshot);
}

#[tokio::test]
async fn native_keyboard_interactive_preserves_echo_flags_and_answers() {
    let server = TestSshServer::start(ServerAuth::KeyboardInteractive).await;
    let endpoint = server.address();
    let temp = TempDir::new().expect("native SSH temp directory");
    let profile = native_profile(server.address(), AuthenticationKind::KeyboardInteractive);
    let auth = AuthPlan::from_profile(&profile, &MemoryCredentialVault::new())
        .expect("keyboard-interactive auth plan");
    let mut transport = native_transport(profile, auth, &temp);

    let (result, prompts) = drive_native_connect(
        &mut transport,
        &TransportRequest::new(size(80, 24)),
        |interaction| match interaction {
            InteractionRequest::HostKey(_) => Some(InteractionResponse::HostKey(
                HostKeyDecision::AcceptAndStore,
            )),
            InteractionRequest::KeyboardInteractive(prompt) => {
                assert_eq!(prompt.name, "Contract authentication");
                assert_eq!(prompt.instruction, "Supply both answers in order");
                assert_eq!(prompt.prompts.len(), 2);
                assert_eq!(prompt.prompts[0].label, "Visible answer");
                assert!(prompt.prompts[0].echo);
                assert_eq!(prompt.prompts[1].label, "One-time code");
                assert!(!prompt.prompts[1].echo);
                Some(InteractionResponse::Answers(
                    KBI_ANSWERS
                        .iter()
                        .map(|answer| SecretString::from((*answer).to_owned()))
                        .collect(),
                ))
            }
            _ => panic!("unexpected SSH interaction"),
        },
    )
    .await;
    result.expect("keyboard-interactive authentication");
    assert_eq!(
        prompts
            .iter()
            .filter(|request| matches!(request, InteractionRequest::KeyboardInteractive(_)))
            .count(),
        1
    );
    assert_keyboard_answers(&server.snapshot());

    let snapshot = shutdown_native(&mut transport, server).await;
    assert_eq!(snapshot.successful_authentications, 1);
    emit_observation_from_snapshot(QaSurface::NativeKeyboardInteractive, endpoint, &snapshot);
}

#[tokio::test]
async fn native_rejected_unknown_host_key_fails_closed() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let endpoint = server.address();
    let temp = TempDir::new().expect("native SSH temp directory");
    let (profile, auth) = vault_plan(
        native_profile(server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    let mut transport = native_transport(profile, auth, &temp);

    let (result, prompts) = drive_native_connect(
        &mut transport,
        &TransportRequest::new(size(80, 24)),
        |interaction| match interaction {
            InteractionRequest::HostKey(_) => {
                Some(InteractionResponse::HostKey(HostKeyDecision::Reject))
            }
            _ => panic!("unexpected SSH interaction"),
        },
    )
    .await;
    let error = result.expect_err("rejected host key must fail the connection");
    assert_eq!(error.failure(), SessionFailure::HostKeyRejected);
    let host_key_prompt_count = prompts
        .iter()
        .filter(|request| matches!(request, InteractionRequest::HostKey(_)))
        .count();
    assert_eq!(host_key_prompt_count, 1);

    let snapshot = shutdown_native(&mut transport, server).await;
    assert_eq!(snapshot.successful_authentications, 0);
    emit_host_key_observation(endpoint, &snapshot, host_key_prompt_count);
}

#[tokio::test]
async fn native_changed_host_key_fails_closed_without_a_prompt() {
    let temp = TempDir::new().expect("native SSH temp directory");
    let trusted_server = TestSshServer::start(ServerAuth::Password).await;
    let address = trusted_server.address();
    let (profile, auth) = vault_plan(
        native_profile(address, AuthenticationKind::Password),
        PASSWORD,
    );
    let mut trusted = native_transport(profile, auth, &temp);
    connect_accepting_host(&mut trusted, &TransportRequest::new(size(80, 24)))
        .await
        .0
        .expect("initial host key confirmation");
    let trusted_snapshot = shutdown_native(&mut trusted, trusted_server).await;
    assert_eq!(trusted_snapshot.successful_authentications, 1);

    let changed_server = TestSshServer::start_at(address, ServerAuth::Password).await;
    let (profile, auth) = vault_plan(
        native_profile(address, AuthenticationKind::Password),
        PASSWORD,
    );
    let mut changed = native_transport(profile, auth, &temp);
    let (result, prompts) =
        connect_accepting_host(&mut changed, &TransportRequest::new(size(80, 24))).await;
    let error = result.expect_err("changed host key must fail the connection");
    assert_eq!(error.failure(), SessionFailure::HostKeyChanged);
    assert!(prompts.is_empty(), "changed key must never be accepted");

    let snapshot = shutdown_native(&mut changed, changed_server).await;
    assert_eq!(snapshot.successful_authentications, 0);
}

#[tokio::test]
async fn native_wrong_password_is_an_authentication_failure_without_secret_output() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let temp = TempDir::new().expect("native SSH temp directory");
    let (profile, auth) = vault_plan(
        native_profile(server.address(), AuthenticationKind::Password),
        WRONG_PASSWORD,
    );
    assert_secret_is_redacted(&auth, WRONG_PASSWORD);
    let mut transport = native_transport(profile, auth, &temp);

    let (result, _) =
        connect_accepting_host(&mut transport, &TransportRequest::new(size(80, 24))).await;
    let error = result.expect_err("wrong password must fail authentication");
    assert_eq!(error.failure(), SessionFailure::Authentication);
    assert_secret_is_redacted(&error, WRONG_PASSWORD);

    let snapshot = shutdown_native(&mut transport, server).await;
    assert_eq!(snapshot.successful_authentications, 0);
}

#[tokio::test]
async fn native_keyboard_interactive_cancel_is_an_authentication_failure() {
    let server = TestSshServer::start(ServerAuth::KeyboardInteractive).await;
    let temp = TempDir::new().expect("native SSH temp directory");
    let profile = native_profile(server.address(), AuthenticationKind::KeyboardInteractive);
    let auth = AuthPlan::from_profile(&profile, &MemoryCredentialVault::new())
        .expect("keyboard-interactive auth plan");
    let mut transport = native_transport(profile, auth, &temp);

    let (result, prompts) = drive_native_connect(
        &mut transport,
        &TransportRequest::new(size(80, 24)),
        |interaction| match interaction {
            InteractionRequest::HostKey(_) => Some(InteractionResponse::HostKey(
                HostKeyDecision::AcceptAndStore,
            )),
            InteractionRequest::KeyboardInteractive(_) => Some(InteractionResponse::Cancel),
            _ => panic!("unexpected SSH interaction"),
        },
    )
    .await;
    let error = result.expect_err("cancelled keyboard-interactive authentication must fail");
    assert_eq!(error.failure(), SessionFailure::Authentication);
    assert_eq!(
        prompts
            .iter()
            .filter(|request| matches!(request, InteractionRequest::KeyboardInteractive(_)))
            .count(),
        1
    );

    let snapshot = shutdown_native(&mut transport, server).await;
    assert_eq!(snapshot.successful_authentications, 0);
}

#[tokio::test]
async fn native_backpressure_drains_ten_kib_then_remains_interactive() {
    let mut burst = vec![b'x'; BACKPRESSURE_BYTES];
    burst.extend_from_slice(BACKPRESSURE_MARKER);
    let server = TestSshServer::start_with_initial_output(ServerAuth::Password, burst).await;
    let temp = TempDir::new().expect("native SSH temp directory");
    let (profile, auth) = vault_plan(
        native_profile(server.address(), AuthenticationKind::Password),
        PASSWORD,
    );
    let mut transport = native_transport(profile, auth, &temp);

    connect_accepting_host(&mut transport, &TransportRequest::new(size(80, 24)))
        .await
        .0
        .expect("backpressure connection");
    let output = output_until(&mut transport, b"SMOKE_BURST_COMPLETE").await;
    assert_eq!(
        output.iter().filter(|byte| **byte == b'x').count(),
        BACKPRESSURE_BYTES,
        "all backpressure bytes must reach the client"
    );
    assert!(
        server.snapshot().emitted_output_bytes
            >= b"READY\r\n".len() + BACKPRESSURE_BYTES + BACKPRESSURE_MARKER.len(),
        "server must record its initial burst"
    );

    transport
        .write(b"backpressure-ack\r\n")
        .await
        .expect("write after draining backpressure");
    assert!(contains(
        &output_until(&mut transport, b"backpressure-ack\r\n").await,
        b"backpressure-ack\r\n"
    ));
    assert!(
        server
            .snapshot()
            .received_input
            .ends_with(b"backpressure-ack\r\n"),
        "server must receive post-backpressure input"
    );

    let snapshot = shutdown_native(&mut transport, server).await;
    assert!(snapshot.emitted_output_bytes >= BACKPRESSURE_BYTES);
}

#[tokio::test]
async fn native_remote_command_preserves_output_nonzero_exit_and_eof_cleanup() {
    let server = TestSshServer::start(ServerAuth::Password).await;
    let temp = TempDir::new().expect("native SSH temp directory");
    let mut profile = native_profile(server.address(), AuthenticationKind::Password);
    profile.remote_command = Some("exit:37".to_owned());
    let (profile, auth) = vault_plan(profile, PASSWORD);
    let mut transport = native_transport(profile, auth, &temp);

    connect_accepting_host(&mut transport, &TransportRequest::new(size(80, 24)))
        .await
        .0
        .expect("remote command authentication");
    let (output, status, eof) = remote_command_events(&mut transport).await;
    assert!(contains(&output, b"remote-output-before-exit\r\n"));
    assert_eq!(status.code, Some(37));
    assert!(!status.success);
    assert!(eof, "remote command must emit EOF");

    let snapshot = shutdown_native(&mut transport, server).await;
    assert_eq!(snapshot.remote_commands, [b"exit:37".to_vec()]);
}

struct AcceptFixtureHostKey;

impl client::Handler for AcceptFixtureHostKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteCommandMessage {
    Data(Vec<u8>),
    ExitStatus(u32),
    Eof,
    Close,
}

async fn connect_fixture_client(address: SocketAddr) -> client::Handle<AcceptFixtureHostKey> {
    let stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect low-level russh fixture client");
    let mut client = client::connect_stream(
        Arc::new(client::Config::default()),
        stream,
        AcceptFixtureHostKey,
    )
    .await
    .expect("start low-level russh fixture client");
    assert!(
        client
            .authenticate_password(USERNAME, PASSWORD)
            .await
            .expect("authenticate low-level russh fixture client")
            .success(),
        "fixture password authentication"
    );
    client
}

async fn fixture_command_messages(
    client: &mut client::Handle<AcceptFixtureHostKey>,
    exit_status: u32,
) -> Vec<RemoteCommandMessage> {
    let mut channel = client
        .channel_open_session()
        .await
        .expect("open low-level russh fixture channel");
    channel
        .exec(true, format!("exit:{exit_status}").into_bytes())
        .await
        .expect("execute low-level russh fixture command");

    tokio::time::timeout(CASE_TIMEOUT, async {
        let mut messages = Vec::new();
        while !matches!(messages.last(), Some(RemoteCommandMessage::Close)) {
            match channel
                .wait()
                .await
                .expect("fixture channel ended before close")
            {
                ChannelMsg::Success => {}
                ChannelMsg::Data { data } => {
                    messages.push(RemoteCommandMessage::Data(data.to_vec()));
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    messages.push(RemoteCommandMessage::ExitStatus(exit_status));
                }
                ChannelMsg::Eof => messages.push(RemoteCommandMessage::Eof),
                ChannelMsg::Close => messages.push(RemoteCommandMessage::Close),
                message => panic!("unexpected fixture channel message: {message:?}"),
            }
        }
        messages
    })
    .await
    .expect("low-level russh fixture command timed out")
}

fn expected_fixture_command_messages(exit_status: u32) -> [RemoteCommandMessage; 4] {
    [
        RemoteCommandMessage::Data(REMOTE_COMMAND_OUTPUT.to_vec()),
        RemoteCommandMessage::ExitStatus(exit_status),
        RemoteCommandMessage::Eof,
        RemoteCommandMessage::Close,
    ]
}

#[tokio::test]
async fn fixture_exec_flushes_close_after_remote_command_termination_batch() {
    let server = TestSshServer::start_observing_exec_flushes(ServerAuth::Password).await;
    let mut client = connect_fixture_client(server.address()).await;
    let messages = fixture_command_messages(&mut client, 37).await;

    assert_eq!(
        messages,
        expected_fixture_command_messages(37),
        "fixture command protocol order"
    );
    assert_eq!(
        server.exec_flush_generations(),
        2,
        "exec termination and close must complete in exactly two post-arm socket flush generations"
    );

    client
        .disconnect(russh::Disconnect::ByApplication, "", "")
        .await
        .expect("disconnect low-level russh fixture client");
    let snapshot = shutdown_server(server).await;
    assert_eq!(snapshot.remote_commands, [b"exit:37".to_vec()]);
}

#[tokio::test]
async fn default_fixture_accepts_sequential_execs_without_observation_limits() {
    let server = TestSshServer::start(ServerAuth::Password).await;

    for exit_status in [11, 12] {
        let mut client = connect_fixture_client(server.address()).await;
        assert_eq!(
            fixture_command_messages(&mut client, exit_status).await,
            expected_fixture_command_messages(exit_status),
            "default fixture command protocol order"
        );
        client
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await
            .expect("disconnect low-level russh fixture client");
    }

    let snapshot = shutdown_server(server).await;
    assert_eq!(snapshot.successful_authentications, 2);
    assert_eq!(
        snapshot.remote_commands,
        [b"exit:11".to_vec(), b"exit:12".to_vec()]
    );
}

#[tokio::test]
#[ignore = "requires the real running system OpenSSH agent; run explicitly with -- --ignored"]
async fn system_openssh_agent_authenticates_against_local_server() {
    let parent_public_key_path =
        std::env::var_os(SYSTEM_AGENT_PUBLIC_KEY_PATH_ENV).map(PathBuf::from);
    let child_agent_key = parent_public_key_path
        .is_none()
        .then(DisposableAgentKey::add)
        .transpose()
        .expect("add disposable key to the running system SSH agent");
    let public_key_path = parent_public_key_path.unwrap_or_else(|| {
        child_agent_key
            .as_ref()
            .expect("child-owned agent key")
            .public_key_path
            .clone()
    });
    let public_key = read_public_key(&public_key_path).expect("read system-agent public key");
    assert_agent_exposes_public_key(&public_key).expect("system agent must expose the QA key");
    let home = TempDir::new().expect("system SSH temporary home");
    std::fs::create_dir_all(home.path().join(".ssh")).expect("system SSH known-hosts directory");
    let _home = EnvRestore::set("HOME", home.path().as_os_str());
    #[cfg(windows)]
    let _user_profile = EnvRestore::set("USERPROFILE", home.path().as_os_str());

    let server = TestSshServer::start(ServerAuth::PublicKey(public_key)).await;
    let endpoint = server.address();
    let mut profile = system_profile(server.address());
    // A public key file selects the matching private key from the real agent
    // without exposing or loading private key bytes in the application.
    profile.identity_file = Some(public_key_path);
    profile.remote_command = Some("exit:0".to_owned());
    let mut transport = SystemOpenSshTransport::new(profile);
    let (broker, _requests): (InteractionBroker, _) = interaction_channel();
    transport
        .connect(&TransportRequest::new(size(80, 24)), broker)
        .await
        .expect("start production system OpenSSH transport");
    let (output, status) = system_remote_command_after_host_confirmation(&mut transport).await;
    assert!(contains_complete_terminal_line(
        &output,
        b"remote-output-before-exit",
    ));
    assert_eq!(status.code, Some(0));
    assert!(status.success);
    tokio::time::timeout(SYSTEM_OPENSSH_TIMEOUT, transport.shutdown())
        .await
        .expect("system OpenSSH shutdown timed out")
        .expect("system OpenSSH shutdown");

    let snapshot = shutdown_server(server).await;
    assert_eq!(snapshot.successful_authentications, 1);
    assert_eq!(snapshot.remote_commands, [b"exit:0".to_vec()]);
    emit_observation_from_snapshot(QaSurface::SystemAgent, endpoint, &snapshot);
    if let Some(agent_key) = child_agent_key {
        agent_key
            .remove()
            .expect("remove only the disposable task key from the system SSH agent");
    }
}

#[test]
fn system_remote_output_accepts_complete_platform_pty_lines_only() {
    let marker = b"remote-output-before-exit";

    assert!(contains_complete_terminal_line(
        b"prompt\nremote-output-before-exit\n",
        marker,
    ));
    assert!(contains_complete_terminal_line(
        b"prompt\r\nremote-output-before-exit\r\n",
        marker,
    ));
    assert!(contains_complete_terminal_line(
        b"prompt\r\r\nremote-output-before-exit\r\r\n",
        marker,
    ));
    assert!(!contains_complete_terminal_line(marker, marker));
    assert!(!contains_complete_terminal_line(
        b"remote-output-before-exit-suffix\n",
        marker,
    ));
    assert!(!contains_complete_terminal_line(
        b"prefix-remote-output-before-exit\n",
        marker,
    ));
}

fn contains_complete_terminal_line(output: &[u8], expected: &[u8]) -> bool {
    output.split_inclusive(|byte| *byte == b'\n').any(|line| {
        let Some(content) = line.strip_suffix(b"\n") else {
            return false;
        };
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        content == expected
    })
}

async fn system_remote_command_after_host_confirmation(
    transport: &mut SystemOpenSshTransport,
) -> (Vec<u8>, ExitStatus) {
    let mut output = Vec::new();
    let mut confirmed = false;
    let mut output_events = 0usize;
    let result = tokio::time::timeout(SYSTEM_OPENSSH_TIMEOUT, async {
        loop {
            match transport.next_event().await.expect("system OpenSSH event") {
                TransportEvent::Output(bytes) => {
                    output_events = output_events.saturating_add(1);
                    output.extend_from_slice(&bytes);
                    if !confirmed
                        && (contains(&output, b"yes/no")
                            || contains(&output, b"yes/no/[fingerprint]"))
                    {
                        transport
                            .write(b"yes\r\n")
                            .await
                            .expect("accept system SSH host key");
                        confirmed = true;
                    }
                }
                TransportEvent::Exit(status) => return status,
                _ => panic!("system OpenSSH ended without an exit status"),
            }
        }
    })
    .await;
    match result {
        Ok(status) => (output, status),
        Err(_) => panic!(
            "system OpenSSH remote command timed out: confirmed={confirmed} output_events={output_events} output_bytes={} marker_seen={} host_prompts={} retry_prompts={} permission_denied={} load_key_error={} agent_refused={}",
            output.len(),
            contains_complete_terminal_line(&output, b"remote-output-before-exit"),
            count_bytes(&output, b"yes/no"),
            count_bytes(&output, b"Please type"),
            contains(&output, b"Permission denied"),
            contains(&output, b"Load key"),
            contains(&output, b"agent refused operation"),
        ),
    }
}

fn count_bytes(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

struct DisposableAgentKey {
    _directory: TempDir,
    public_key_path: PathBuf,
    public_key: PublicKey,
    added: bool,
}

impl DisposableAgentKey {
    fn add() -> Result<Self, String> {
        let directory = TempDir::new().map_err(|error| error.to_string())?;
        let private_key_path = directory.path().join("rshell-ssh-smoke-agent-key");
        let generated = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&private_key_path)
            .output()
            .map_err(|_| "system SSH agent smoke requires the ssh-keygen executable".to_owned())?;
        if !generated.status.success() {
            return Err(
                "system SSH agent smoke could not generate a disposable task key".to_owned(),
            );
        }
        let public_key_path = private_key_path.with_extension("pub");
        let public_key = read_public_key(&public_key_path)?;
        let added = Command::new("ssh-add")
            .arg(&private_key_path)
            .output()
            .map_err(|_| {
                "system SSH agent smoke requires the ssh-add executable and a running agent"
                    .to_owned()
            })?;
        if !added.status.success() {
            return Err(
                "system SSH agent smoke requires the real running OpenSSH agent to accept a disposable task key"
                    .to_owned(),
            );
        }
        let key = Self {
            _directory: directory,
            public_key_path,
            public_key,
            added: true,
        };
        key.assert_loaded()?;
        Ok(key)
    }

    fn assert_loaded(&self) -> Result<(), String> {
        assert_agent_exposes_public_key(&self.public_key)
    }

    fn remove(mut self) -> Result<(), String> {
        self.remove_inner()?;
        self.added = false;
        Ok(())
    }

    fn remove_inner(&self) -> Result<(), String> {
        let removed = Command::new("ssh-add")
            .arg("-d")
            .arg(&self.public_key_path)
            .output()
            .map_err(|_| {
                "system SSH agent smoke could not remove its disposable task key".to_owned()
            })?;
        if !removed.status.success() {
            return Err(
                "system SSH agent smoke failed to remove its disposable task key from the running agent"
                    .to_owned(),
            );
        }
        let remaining = Command::new("ssh-add").arg("-L").output().map_err(|_| {
            "system SSH agent smoke could not verify disposable task key removal".to_owned()
        })?;
        if remaining.status.success()
            && String::from_utf8_lossy(&remaining.stdout)
                .lines()
                .filter_map(parse_public_key_line)
                .any(|key| key == self.public_key)
        {
            return Err(
                "system SSH agent smoke left its disposable task key loaded in the running agent"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

fn assert_agent_exposes_public_key(expected: &PublicKey) -> Result<(), String> {
    let output = Command::new("ssh-add")
        .arg("-L")
        .output()
        .map_err(|_| "system SSH agent smoke could not inspect agent identities".to_owned())?;
    if output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_public_key_line)
            .any(|key| key == *expected)
    {
        Ok(())
    } else {
        Err("system SSH agent smoke did not expose the expected task identity".to_owned())
    }
}

impl Drop for DisposableAgentKey {
    fn drop(&mut self) {
        if self.added
            && let Err(error) = self.remove_inner()
            && !std::thread::panicking()
        {
            panic!("{error}");
        }
    }
}

fn read_public_key(path: &Path) -> Result<PublicKey, String> {
    let contents = fs::read_to_string(path).map_err(|_| {
        "system SSH agent smoke could not read its disposable task public key".to_owned()
    })?;
    parse_public_key_line(contents.trim()).ok_or_else(|| {
        "system SSH agent smoke could not parse its disposable task public key".to_owned()
    })
}

fn parse_public_key_line(line: &str) -> Option<PublicKey> {
    line.split_whitespace()
        .nth(1)
        .and_then(|encoded| parse_public_key_base64(encoded).ok())
}

#[tokio::test]
#[ignore = "starts the local russh fixture server for the PowerShell P0 parent harness; run explicitly with -- --ignored"]
async fn local_russh_smoke_fixture_server() {
    if std::env::var_os("RSHELL_QA_INJECT_FAIL_BEFORE_READY").is_some() {
        panic!("intentional fail_before_fixture_ready before server mutation");
    }
    let ready_path = required_env_path(FIXTURE_READY_PATH_ENV);
    let stop_path = required_env_path(FIXTURE_STOP_PATH_ENV);
    let observation_dir = required_env_path(FIXTURE_OBSERVATION_DIR_ENV);
    assert!(
        observation_dir.is_dir(),
        "{FIXTURE_OBSERVATION_DIR_ENV} must name an existing directory"
    );
    assert!(
        ready_path.parent().is_some_and(Path::is_dir),
        "{FIXTURE_READY_PATH_ENV} parent directory must exist"
    );
    assert!(
        stop_path.parent().is_some_and(Path::is_dir),
        "{FIXTURE_STOP_PATH_ENV} parent directory must exist"
    );
    assert!(
        !ready_path.exists(),
        "{FIXTURE_READY_PATH_ENV} must not already exist; use a UUID-controlled path"
    );
    assert!(
        !stop_path.exists(),
        "{FIXTURE_STOP_PATH_ENV} must not already exist; use a UUID-controlled path"
    );

    let password_env_name = std::env::var(FIXTURE_PASSWORD_ENV_NAME_ENV).unwrap_or_else(|_| {
        panic!(
            "{FIXTURE_PASSWORD_ENV_NAME_ENV} must name the environment variable containing the fixture password"
        )
    });
    assert!(
        is_environment_name(&password_env_name),
        "{FIXTURE_PASSWORD_ENV_NAME_ENV} must be an environment variable name"
    );
    let password = std::env::var(&password_env_name).unwrap_or_else(|_| {
        panic!("the fixture password environment variable named by {FIXTURE_PASSWORD_ENV_NAME_ENV} is missing")
    });
    assert!(!password.is_empty(), "fixture password must not be empty");
    let expected_surfaces = fixture_expected_surfaces();
    let run_nonce = required_binding_value(FIXTURE_RUN_NONCE_ENV);
    let fixture_id = required_binding_value(FIXTURE_ID_ENV);
    let key_passphrase_env_name = required_secret_env_name(FIXTURE_KEY_PASSPHRASE_ENV_NAME_ENV);
    assert!(
        !std::env::var(&key_passphrase_env_name)
            .unwrap_or_else(|_| panic!(
                "the environment variable named by {FIXTURE_KEY_PASSPHRASE_ENV_NAME_ENV} is missing"
            ))
            .is_empty(),
        "fixture encrypted-key passphrase must not be empty"
    );
    let kbi_visible_answer_env_name =
        required_secret_env_name(FIXTURE_KBI_VISIBLE_ANSWER_ENV_NAME_ENV);
    assert!(
        std::env::var(&kbi_visible_answer_env_name).unwrap_or_else(|_| {
            panic!(
                "the environment variable named by {FIXTURE_KBI_VISIBLE_ANSWER_ENV_NAME_ENV} is missing"
            )
        }) == KBI_ANSWERS[0],
        "fixture keyboard-interactive visible answer must match its server"
    );
    let kbi_one_time_code_env_name =
        required_secret_env_name(FIXTURE_KBI_ONE_TIME_CODE_ENV_NAME_ENV);
    assert!(
        std::env::var(&kbi_one_time_code_env_name).unwrap_or_else(|_| {
            panic!(
                "the environment variable named by {FIXTURE_KBI_ONE_TIME_CODE_ENV_NAME_ENV} is missing"
            )
        }) == KBI_ANSWERS[1],
        "fixture keyboard-interactive one-time code must match its server"
    );

    let encrypted_key_path = required_env_path(FIXTURE_ENCRYPTED_KEY_PATH_ENV);
    assert!(
        encrypted_key_path.parent().is_some_and(Path::is_dir),
        "{FIXTURE_ENCRYPTED_KEY_PATH_ENV} parent directory must exist"
    );
    assert!(
        encrypted_key_path.is_file(),
        "{FIXTURE_ENCRYPTED_KEY_PATH_ENV} must name the parent-owned encrypted private key"
    );
    let encrypted_key = read_public_key(&encrypted_key_path.with_extension("pub"))
        .expect("read the parent-owned encrypted client public key");

    let agent_public_key_path = required_env_path(FIXTURE_AGENT_PUBLIC_KEY_PATH_ENV);
    let agent_public_key = read_public_key(&agent_public_key_path)
        .expect("read the parent-owned fixture agent public key");
    let native_password = if expected_surfaces.contains(&QaSurface::NativePassword) {
        Some(TestSshServer::start(ServerAuth::PasswordValue(password.clone())).await)
    } else {
        None
    };
    let native_key = if expected_surfaces.contains(&QaSurface::NativeKey) {
        Some(TestSshServer::start(ServerAuth::PublicKey(encrypted_key)).await)
    } else {
        None
    };
    let native_keyboard_interactive =
        if expected_surfaces.contains(&QaSurface::NativeKeyboardInteractive) {
            Some(TestSshServer::start(ServerAuth::KeyboardInteractive).await)
        } else {
            None
        };
    let system_agent = if expected_surfaces.contains(&QaSurface::SystemAgent) {
        Some(TestSshServer::start(ServerAuth::PublicKey(agent_public_key)).await)
    } else {
        None
    };
    let host_key = if expected_surfaces.contains(&QaSurface::HostKey) {
        Some(TestSshServer::start(ServerAuth::PasswordValue(password)).await)
    } else {
        None
    };
    write_fixture_ready_document(
        &ready_path,
        FixtureReady {
            endpoints: FixtureEndpoints {
                native_password: native_password.as_ref().map(TestSshServer::address),
                native_key: native_key.as_ref().map(TestSshServer::address),
                native_keyboard_interactive: native_keyboard_interactive
                    .as_ref()
                    .map(TestSshServer::address),
                system_agent: system_agent.as_ref().map(TestSshServer::address),
                host_key: host_key.as_ref().map(TestSshServer::address),
            },
            encrypted_key_path: &encrypted_key_path,
            agent_public_key_path: expected_surfaces
                .contains(&QaSurface::SystemAgent)
                .then_some(agent_public_key_path.as_path()),
            observation_dir: &observation_dir,
            password_env_name: &password_env_name,
            key_passphrase_env_name: &key_passphrase_env_name,
            kbi_visible_answer_env_name: &kbi_visible_answer_env_name,
            kbi_one_time_code_env_name: &kbi_one_time_code_env_name,
            run_nonce: &run_nonce,
            fixture_id: &fixture_id,
        },
    );
    wait_for_fixture_observations_and_stop(
        &stop_path,
        &observation_dir,
        &expected_surfaces,
        &run_nonce,
        &fixture_id,
        &[
            (QaSurface::NativePassword, native_password.as_ref()),
            (QaSurface::NativeKey, native_key.as_ref()),
            (
                QaSurface::NativeKeyboardInteractive,
                native_keyboard_interactive.as_ref(),
            ),
            (QaSurface::SystemAgent, system_agent.as_ref()),
            (QaSurface::HostKey, host_key.as_ref()),
        ],
    )
    .await;

    let native_password = shutdown_optional_server(native_password).await;
    let native_key = shutdown_optional_server(native_key).await;
    let native_keyboard_interactive = shutdown_optional_server(native_keyboard_interactive).await;
    let system_agent = shutdown_optional_server(system_agent).await;
    let host_key = shutdown_optional_server(host_key).await;
    if std::env::var_os("RSHELL_QA_INJECT_FINAL_FAILURE").is_some() {
        panic!("intentional fixture final assertions failure after exact server shutdown");
    }
    assert_fixture_observations(
        &observation_dir,
        &[
            (QaSurface::NativePassword, native_password.as_ref()),
            (QaSurface::NativeKey, native_key.as_ref()),
            (
                QaSurface::NativeKeyboardInteractive,
                native_keyboard_interactive.as_ref(),
            ),
            (QaSurface::SystemAgent, system_agent.as_ref()),
            (QaSurface::HostKey, host_key.as_ref()),
        ],
        &expected_surfaces,
    );
    for surface in expected_surfaces {
        assert!(
            observation_dir
                .join(format!("{}.json", surface.as_str()))
                .is_file(),
            "{} fixture observation was not produced from asserted server facts",
            surface.as_str()
        );
    }
}

fn required_env_path(name: &'static str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must be set to a UUID-controlled path"))
}

fn required_secret_env_name(name: &'static str) -> String {
    let secret_env_name = std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must name an environment variable containing a secret"));
    assert!(
        is_environment_name(&secret_env_name),
        "{name} must be an environment variable name"
    );
    secret_env_name
}

fn required_binding_value(name: &'static str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"));
    assert!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "{name} must be a non-secret binding label"
    );
    value
}

fn fixture_expected_surfaces() -> Vec<QaSurface> {
    let configured = std::env::var(FIXTURE_EXPECTED_SURFACES_ENV).unwrap_or_else(|_| {
        panic!(
            "{FIXTURE_EXPECTED_SURFACES_ENV} must list the comma-separated QA surfaces exercised by the parent harness"
        )
    });
    let mut expected = configured
        .split(',')
        .map(|surface| match surface {
            "native_password" => QaSurface::NativePassword,
            "native_key" => QaSurface::NativeKey,
            "native_keyboard_interactive" => QaSurface::NativeKeyboardInteractive,
            "system_agent" => QaSurface::SystemAgent,
            "host_key" => QaSurface::HostKey,
            _ => panic!("{FIXTURE_EXPECTED_SURFACES_ENV} contains an unknown QA surface"),
        })
        .collect::<Vec<_>>();
    expected.sort_by_key(|surface| surface.as_str());
    expected.dedup_by_key(|surface| surface.as_str());
    assert!(
        !expected.is_empty(),
        "{FIXTURE_EXPECTED_SURFACES_ENV} must include at least one QA surface"
    );
    expected
}

async fn shutdown_optional_server(server: Option<TestSshServer>) -> Option<ServerSnapshot> {
    match server {
        Some(server) => Some(shutdown_server(server).await),
        None => None,
    }
}

fn is_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

async fn wait_for_fixture_observations_and_stop(
    stop_path: &Path,
    observation_dir: &Path,
    expected_surfaces: &[QaSurface],
    run_nonce: &str,
    fixture_id: &str,
    servers: &[(QaSurface, Option<&TestSshServer>)],
) {
    tokio::time::timeout(FIXTURE_WAIT_TIMEOUT, async {
        loop {
            for (surface, server) in servers {
                if !expected_surfaces.contains(surface) {
                    continue;
                }
                let path = observation_dir.join(format!("{}.json", surface.as_str()));
                let server = server.unwrap_or_else(|| {
                    panic!("{} fixture server was not started", surface.as_str())
                });
                let snapshot = server.snapshot();
                if !path.exists() && fixture_surface_facts_ready(*surface, &snapshot) {
                    write_observation_document(
                        &path,
                        *surface,
                        authenticated_channel_observations(*surface),
                        ObservationBinding {
                            run_nonce,
                            fixture: fixture_id,
                            connection: surface.as_str(),
                            endpoint: server.address().to_string(),
                        },
                    );
                }
            }
            if stop_path.is_file() {
                return;
            }
            tokio::time::sleep(FIXTURE_POLL_INTERVAL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "local russh fixture server timed out waiting for {}",
            stop_path.display()
        )
    });
}

struct FixtureEndpoints {
    native_password: Option<SocketAddr>,
    native_key: Option<SocketAddr>,
    native_keyboard_interactive: Option<SocketAddr>,
    system_agent: Option<SocketAddr>,
    host_key: Option<SocketAddr>,
}

struct FixtureReady<'a> {
    endpoints: FixtureEndpoints,
    encrypted_key_path: &'a Path,
    agent_public_key_path: Option<&'a Path>,
    observation_dir: &'a Path,
    password_env_name: &'a str,
    key_passphrase_env_name: &'a str,
    kbi_visible_answer_env_name: &'a str,
    kbi_one_time_code_env_name: &'a str,
    run_nonce: &'a str,
    fixture_id: &'a str,
}

fn write_fixture_ready_document(ready_path: &Path, ready: FixtureReady<'_>) {
    let encrypted_key_path = json_path(ready.encrypted_key_path);
    let agent_public_key_path = ready.agent_public_key_path.map_or_else(
        || "null".to_owned(),
        |path| format!("\"{}\"", json_path(path)),
    );
    let observation_dir = json_path(ready.observation_dir);
    let document = format!(
        concat!(
            "{{\"version\":1,\"generated_by\":\"p0_qa\",\"username\":\"{}\",",
            "\"run_nonce\":\"{}\",\"fixture\":\"{}\",",
            "\"password_env\":\"{}\",\"key_passphrase_env\":\"{}\",",
            "\"keyboard_interactive_visible_answer_env\":\"{}\",",
            "\"keyboard_interactive_one_time_code_env\":\"{}\",",
            "\"encrypted_key_path\":\"{}\",\"agent_public_key_path\":{},",
            "\"observation_dir\":\"{}\",\"endpoints\":{{",
            "\"native_password\":{},\"native_key\":{},",
            "\"native_keyboard_interactive\":{},\"system_agent\":{},\"host_key\":{}}}}}\n"
        ),
        USERNAME,
        ready.run_nonce,
        ready.fixture_id,
        ready.password_env_name,
        ready.key_passphrase_env_name,
        ready.kbi_visible_answer_env_name,
        ready.kbi_one_time_code_env_name,
        encrypted_key_path,
        agent_public_key_path,
        observation_dir,
        endpoint_json(ready.endpoints.native_password),
        endpoint_json(ready.endpoints.native_key),
        endpoint_json(ready.endpoints.native_keyboard_interactive),
        endpoint_json(ready.endpoints.system_agent),
        endpoint_json(ready.endpoints.host_key),
    );
    atomic_write(ready_path, &document, "fixture readiness");
}

fn endpoint_json(address: Option<SocketAddr>) -> String {
    address.map_or_else(
        || "null".to_owned(),
        |address| {
            format!(
                "{{\"host\":\"{}\",\"port\":{}}}",
                address.ip(),
                address.port()
            )
        },
    )
}

fn assert_fixture_observations(
    observation_dir: &Path,
    snapshots: &[(QaSurface, Option<&ServerSnapshot>)],
    expected_surfaces: &[QaSurface],
) {
    for (surface, snapshot) in snapshots {
        if !expected_surfaces.contains(surface) {
            continue;
        }
        let snapshot = snapshot.unwrap_or_else(|| {
            panic!(
                "{} fixture server was not started for an expected QA surface",
                surface.as_str()
            )
        });
        assert_fixture_surface_observation_facts(*surface, snapshot);
        assert!(
            observation_dir
                .join(format!("{}.json", surface.as_str()))
                .is_file(),
            "{} bound observation was not written before fixture stop",
            surface.as_str()
        );
    }
}

fn fixture_surface_facts_ready(surface: QaSurface, snapshot: &ServerSnapshot) -> bool {
    let clean = snapshot.active_sessions == 0 && snapshot.open_channels == 0;
    if surface == QaSurface::HostKey {
        return clean
            && snapshot.accepted_connections > 0
            && snapshot.successful_authentications == 0
            && snapshot.opened_channels == 0;
    }
    let method = match surface {
        QaSurface::NativePassword => snapshot.password_authentications > 0,
        QaSurface::NativeKey | QaSurface::SystemAgent => snapshot.public_key_authentications > 0,
        QaSurface::NativeKeyboardInteractive => snapshot.keyboard_interactive_authentications > 0,
        QaSurface::HostKey => false,
    };
    clean && method && snapshot.successful_authentications > 0 && snapshot.opened_channels > 0
}

fn authenticated_channel_observations(surface: QaSurface) -> &'static [&'static str] {
    match surface {
        QaSurface::HostKey => &["server_host_key_prompt"],
        QaSurface::NativePassword
        | QaSurface::NativeKey
        | QaSurface::NativeKeyboardInteractive
        | QaSurface::SystemAgent => &["server_authentication", "server_channel"],
    }
}

fn assert_fixture_surface_observation_facts(surface: QaSurface, snapshot: &ServerSnapshot) {
    if surface == QaSurface::HostKey {
        assert!(
            snapshot.accepted_connections > 0,
            "host-key client case made no server connection"
        );
        assert_eq!(
            snapshot.successful_authentications, 0,
            "host-key rejection must precede authentication"
        );
        assert_eq!(
            snapshot.opened_channels, 0,
            "host-key rejection must precede channel open"
        );
        assert_eq!(
            snapshot.active_sessions, 0,
            "host-key client case leaked server session"
        );
        assert_eq!(
            snapshot.open_channels, 0,
            "host-key client case leaked server channel"
        );
        return;
    }
    assert!(
        snapshot.successful_authentications > 0,
        "{} client case asserted no server authentication",
        surface.as_str()
    );
    assert!(
        snapshot.opened_channels > 0,
        "{} client case asserted no server channel",
        surface.as_str()
    );
    assert_eq!(
        snapshot.active_sessions,
        0,
        "{} client case leaked server session",
        surface.as_str()
    );
    assert_eq!(
        snapshot.open_channels,
        0,
        "{} client case leaked server channel",
        surface.as_str()
    );
    match surface {
        QaSurface::NativePassword => assert!(
            snapshot.password_authentications > 0,
            "{} fixture server recorded no password authentication",
            surface.as_str()
        ),
        QaSurface::NativeKey | QaSurface::SystemAgent => assert!(
            snapshot.public_key_authentications > 0,
            "{} fixture server recorded no public-key authentication",
            surface.as_str()
        ),
        QaSurface::NativeKeyboardInteractive => {
            assert!(
                snapshot.keyboard_interactive_authentications > 0,
                "keyboard-interactive fixture server recorded no keyboard-interactive authentication"
            );
            assert_keyboard_answers(snapshot);
        }
        QaSurface::HostKey => unreachable!("host key facts return before authentication checks"),
    }
}

fn atomic_write(path: &Path, document: &str, label: &str) {
    let parent = path
        .parent()
        .expect("atomic-write path must have a parent directory");
    assert!(
        parent.is_dir(),
        "{label} directory must exist: {}",
        parent.display()
    );
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .expect("atomic-write file name must be UTF-8"),
        std::process::id(),
        sequence
    ));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(document.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        panic!(
            "failed to atomically write {label} to {}: {error}",
            path.display()
        );
    }
}

fn json_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

struct EnvRestore {
    name: &'static str,
    original: Option<OsString>,
}

impl EnvRestore {
    fn set(name: &'static str, value: impl Into<OsString>) -> Self {
        let original = std::env::var_os(name);
        unsafe { std::env::set_var(name, value.into()) };
        Self { name, original }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
