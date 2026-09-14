use std::{
    ffi::OsString,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rshell_core::{
    AuthenticationKind, ConnectionProfile, ExitStatus, SessionFailure, TerminalSize,
};
use rshell_session::{
    SessionTransport, SystemOpenSshTransport, TransportCapabilities, TransportEvent,
    TransportRequest, build_system_ssh_argv, interaction_channel,
};

const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

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

fn profile(
    username: &str,
    host: &str,
    port: u16,
    remote_command: Option<&str>,
) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new("System OpenSSH test", host);
    profile.username = username.to_owned();
    profile.port = port;
    profile.authentication = AuthenticationKind::Agent;
    profile.remote_command = remote_command.map(str::to_owned);
    profile
}

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 800,
        pixel_height: 480,
        dpi: 96,
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pty_echo"))
}

async fn connect(transport: &mut SystemOpenSshTransport) {
    let (interactions, _requests) = interaction_channel();
    transport
        .connect(&TransportRequest::new(size()), interactions)
        .await
        .expect("system OpenSSH transport should start the fake executable");
}

async fn read_until(transport: &mut SystemOpenSshTransport, needle: &[u8]) -> Vec<u8> {
    tokio::time::timeout(EVENT_TIMEOUT, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("system SSH event") {
                TransportEvent::Output(bytes) => {
                    output.extend_from_slice(&bytes);
                    if contains(&output, needle) {
                        return output;
                    }
                }
                TransportEvent::Exit(status) => {
                    panic!("fake SSH exited before output marker: {status:?}")
                }
                event => panic!("unexpected system SSH event: {event:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for fake SSH output")
}

async fn read_to_exit(transport: &mut SystemOpenSshTransport) -> (Vec<u8>, ExitStatus) {
    tokio::time::timeout(EVENT_TIMEOUT, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("system SSH event") {
                TransportEvent::Output(bytes) => output.extend_from_slice(&bytes),
                TransportEvent::Exit(status) => return (output, status),
                event => panic!("unexpected system SSH event: {event:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for fake SSH exit")
}

#[test]
fn argv_is_strict_separate_and_places_destination_after_option_terminator() {
    let argv = build_system_ssh_argv(&profile("user", "host", 2222, Some("printf 'a b'")))
        .expect("valid profile");

    assert_eq!(
        argv,
        [
            "-p",
            "2222",
            "-o",
            "StrictHostKeyChecking=ask",
            "--",
            "user@host",
            "printf 'a b'",
        ]
        .map(OsString::from)
        .to_vec()
    );
}

#[test]
fn argv_uses_host_without_user_when_username_is_empty() {
    let argv = build_system_ssh_argv(&profile("", "host", 22, None)).expect("valid profile");

    assert_eq!(
        argv,
        ["-o", "StrictHostKeyChecking=ask", "--", "host"]
            .map(OsString::from)
            .to_vec()
    );
}

#[test]
fn argv_adds_identity_as_distinct_arguments() {
    let mut profile = profile("user", "host", 22, None);
    profile.identity_file = Some(PathBuf::from("key path;not-a-command"));

    assert_eq!(
        build_system_ssh_argv(&profile).expect("valid profile"),
        [
            "-i",
            "key path;not-a-command",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=ask",
            "--",
            "user@host",
        ]
        .map(OsString::from)
        .to_vec()
    );
}

#[test]
fn option_like_host_nul_and_newline_are_rejected_not_escaped() {
    for host in ["-oProxyCommand=bad", "good\nProxyCommand bad", "a\0b"] {
        let error = build_system_ssh_argv(&profile("user", host, 22, None))
            .expect_err("unsafe host must be rejected");
        assert_eq!(error.failure(), SessionFailure::Validation);
    }
}

#[test]
fn user_identity_and_remote_command_control_boundaries_are_rejected() {
    let mut invalid_user = profile("bad\ruser", "host", 22, None);
    assert_eq!(
        build_system_ssh_argv(&invalid_user)
            .expect_err("CR in user must be rejected")
            .failure(),
        SessionFailure::Validation
    );

    invalid_user.username = "user".to_owned();
    invalid_user.identity_file = Some(PathBuf::from("identity\nfile"));
    assert_eq!(
        build_system_ssh_argv(&invalid_user)
            .expect_err("newline in identity path must be rejected")
            .failure(),
        SessionFailure::Validation
    );

    invalid_user.identity_file = None;
    invalid_user.remote_command = Some("printf ok\0bad".to_owned());
    assert_eq!(
        build_system_ssh_argv(&invalid_user)
            .expect_err("NUL in remote command must be rejected")
            .failure(),
        SessionFailure::Validation
    );
}

#[test]
fn system_transport_advertises_agent_but_not_managed_password() {
    let transport = SystemOpenSshTransport::new(profile("user", "host", 22, None));

    assert_eq!(
        transport.capabilities(),
        TransportCapabilities {
            agent: true,
            public_key: true,
            managed_password: false,
            keyboard_interactive: false,
            host_key_prompt: true,
        }
    );
}

#[tokio::test]
async fn fake_ssh_receives_literal_argv_and_clean_eof_preserves_output() {
    let _ssh = EnvRestore::set("RSHELL_SSH", fixture().as_os_str());
    let sentinel = unique_temp_path("rshell system ssh injection");
    let identity = format!("identity path; no shell {}", sentinel.display());
    let remote_command = format!("printf 'a b'; echo \"quoted\" > {}", sentinel.display());
    let mut profile = profile("user", "host", 2202, Some(&remote_command));
    profile.identity_file = Some(PathBuf::from(&identity));
    let expected = build_system_ssh_argv(&profile).expect("valid fake SSH profile");
    let mut transport = SystemOpenSshTransport::new(profile);

    connect(&mut transport).await;
    let pid = transport
        .process_id()
        .expect("system OpenSSH PTY must expose its real child PID");
    let output = read_until(&mut transport, remote_command.as_bytes()).await;
    for (index, argument) in expected.iter().enumerate() {
        let recorded = format!("ARG:{index}:{}", argument.to_string_lossy());
        assert!(
            contains(&output, recorded.as_bytes()),
            "fake SSH did not receive literal argument {index}: {argument:?}"
        );
    }
    assert!(
        !sentinel.exists(),
        "a profile argument was interpreted by a local shell"
    );

    transport
        .write(b"quit\r\n")
        .await
        .expect("request clean exit");
    let (tail, status) = read_to_exit(&mut transport).await;
    assert!(contains(&tail, b"CLEAN_EXIT"));
    assert_eq!(
        status,
        ExitStatus {
            code: Some(0),
            success: true,
        }
    );
    transport
        .shutdown()
        .await
        .expect("shutdown after clean EOF");
    assert!(
        !process_is_active(pid),
        "system OpenSSH child {pid} survived shutdown"
    );
}

#[tokio::test]
async fn fake_ssh_reports_exact_nonzero_exit() {
    let _ssh = EnvRestore::set("RSHELL_SSH", fixture().as_os_str());
    let mut transport = SystemOpenSshTransport::new(profile("user", "host", 22, None));

    connect(&mut transport).await;
    let _output = read_until(&mut transport, b"READY").await;
    transport
        .write(b"exit:29\r\n")
        .await
        .expect("request nonzero exit");
    let (output, status) = read_to_exit(&mut transport).await;
    assert!(contains(&output, b"BEFORE_EXIT:29"));
    assert_eq!(
        status,
        ExitStatus {
            code: Some(29),
            success: false,
        }
    );
    transport
        .shutdown()
        .await
        .expect("shutdown after nonzero exit");
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn unique_temp_path(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()))
}

#[cfg(unix)]
fn process_is_active(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    i32::try_from(pid).is_ok_and(|pid| unsafe { kill(pid, 0) } == 0)
}

#[cfg(windows)]
fn process_is_active(pid: u32) -> bool {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    let mut exit_code = 0;
    let active =
        unsafe { GetExitCodeProcess(process, &mut exit_code) } != 0 && exit_code == STILL_ACTIVE;
    unsafe {
        CloseHandle(process);
    }
    active
}
