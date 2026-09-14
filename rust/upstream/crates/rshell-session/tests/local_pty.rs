use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rshell_core::{ExitStatus, SessionFailure, TerminalSize};
use rshell_session::{
    LocalLaunch, LocalPtyFactory, LocalPtyTransport, SessionTransport, TransportEvent,
    TransportFactory, TransportRequest, interaction_channel,
};

const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pty_echo"))
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

fn command(args: impl IntoIterator<Item = impl Into<OsString>>) -> LocalLaunch {
    LocalLaunch::Command {
        program: fixture(),
        args: args.into_iter().map(Into::into).collect(),
        cwd: None,
        env: BTreeMap::new(),
    }
}

async fn connect(transport: &mut LocalPtyTransport, request: &TransportRequest) {
    let (interactions, _requests) = interaction_channel();
    SessionTransport::connect(transport, request, interactions)
        .await
        .expect("local PTY connect should succeed");
}

async fn read_until(transport: &mut LocalPtyTransport, needle: &[u8]) -> Vec<u8> {
    tokio::time::timeout(EVENT_TIMEOUT, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("PTY event") {
                TransportEvent::Output(bytes) => {
                    output.extend_from_slice(&bytes);
                    if contains(&output, needle) {
                        return output;
                    }
                }
                TransportEvent::Exit(status) => {
                    panic!("child exited before output marker: {status:?}")
                }
                event => panic!("unexpected local PTY event: {event:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for PTY output")
}

async fn read_to_exit(transport: &mut LocalPtyTransport) -> (Vec<u8>, ExitStatus) {
    tokio::time::timeout(EVENT_TIMEOUT, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("PTY event") {
                TransportEvent::Output(bytes) => output.extend_from_slice(&bytes),
                TransportEvent::Exit(status) => return (output, status),
                event => panic!("unexpected local PTY event: {event:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for child exit")
}

#[tokio::test]
async fn real_pty_preserves_output_resizes_and_shuts_down_idempotently() {
    let request = TransportRequest::new(size(80, 24));
    let mut transport =
        LocalPtyTransport::launch(command(["--watch-resize", "--split-watch-marker"]));
    connect(&mut transport, &request).await;
    let startup = read_until(&mut transport, b"READY").await;
    assert!(contains(&startup, b"TERM:xterm-256color"));
    assert!(contains(&startup, b"INITIAL_SIZE:80x24"));
    transport.write(b"").await.expect("empty write");

    transport
        .write(b"hello:hello world\r\n")
        .await
        .expect("write to PTY");
    let output = read_until(&mut transport, b"WATCHING_SIZE").await;
    assert!(contains(&output, "\u{1b}[31mCOLOR".as_bytes()));
    assert!(contains(&output, "WIDE:界🙂".as_bytes()));
    assert!(contains(&output, b"ECHO:hello world"));
    assert!(contains(&output, b"WATCHING_SIZE"));

    transport
        .resize(size(100, 30))
        .await
        .expect("resize real PTY");
    let resized = read_until(&mut transport, b"SIZE:100x30").await;
    assert!(contains(&resized, b"SIZE:100x30"));

    let (tail, status) = read_to_exit(&mut transport).await;
    assert!(!contains(&tail, b"BEFORE_EXIT:"));
    assert_eq!(
        status,
        ExitStatus {
            code: Some(0),
            success: true
        }
    );
    transport.shutdown().await.expect("first shutdown");
    transport.shutdown().await.expect("repeated shutdown");
}

#[tokio::test]
async fn nonzero_exit_is_exact_and_output_before_eof_is_retained() {
    let mut transport = LocalPtyTransport::launch(command(["--exit", "23"]));
    connect(&mut transport, &TransportRequest::new(size(80, 24))).await;
    let (output, status) = read_to_exit(&mut transport).await;
    assert!(contains(&output, b"BEFORE_EXIT:23"));
    assert_eq!(
        status,
        ExitStatus {
            code: Some(23),
            success: false
        }
    );
    transport.shutdown().await.expect("shutdown after EOF");
}

#[tokio::test]
async fn argv_cwd_env_and_term_are_passed_without_shell_parsing() {
    let directory = unique_temp_path("rshell pty cwd");
    fs::create_dir(&directory).expect("create fixture cwd");
    let sentinel = directory.join("shell-injection-sentinel");
    let injection = format!("& echo injected > {}", sentinel.display());
    let args = vec![OsString::from("space value"), OsString::from(&injection)];
    let mut env = BTreeMap::new();
    env.insert(
        OsString::from("RSHELL_FIXTURE_ENV"),
        OsString::from("value with spaces"),
    );
    let launch = LocalLaunch::Command {
        program: fixture(),
        args,
        cwd: Some(directory.clone()),
        env,
    };
    let request = TransportRequest::new(size(80, 24))
        .with_terminal_type("screen-256color")
        .expect("valid terminal type");
    let mut transport = LocalPtyTransport::launch(launch);
    connect(&mut transport, &request).await;
    let output = read_until(&mut transport, b"READY").await;

    assert!(contains(&output, b"ARG:0:space value"));
    assert!(contains(&output, format!("ARG:1:{injection}").as_bytes()));
    let text = String::from_utf8_lossy(&output);
    let child_cwd = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .find_map(|line| line.strip_prefix("CWD:"))
        .expect("fixture must report its current directory");
    assert_eq!(
        fs::canonicalize(child_cwd).expect("canonical child cwd"),
        fs::canonicalize(&directory).expect("canonical requested cwd")
    );
    assert!(contains(&output, b"ENV:value with spaces"));
    assert!(contains(&output, b"TERM:screen-256color"));
    assert!(!sentinel.exists(), "an argument was interpreted by a shell");

    transport.write(b"quit\r\n").await.expect("request exit");
    let (_, status) = read_to_exit(&mut transport).await;
    assert!(status.success);
    transport.shutdown().await.expect("shutdown");
    fs::remove_dir_all(directory).expect("remove fixture cwd");
}

#[tokio::test]
async fn invalid_size_and_terminal_type_are_rejected_before_spawn() {
    let sentinel = unique_temp_path("rshell-pty-must-not-spawn");
    let mut transport = LocalPtyTransport::launch(command([
        OsString::from("--touch"),
        sentinel.as_os_str().to_owned(),
    ]));
    let zero = TerminalSize {
        cols: 0,
        ..size(80, 24)
    };
    let (interactions, _requests) = interaction_channel();
    let error = transport
        .connect(&TransportRequest::new(zero), interactions)
        .await
        .expect_err("zero width must fail");
    assert_eq!(error.failure(), SessionFailure::Validation);
    assert!(!sentinel.exists());

    let overflow = TerminalSize {
        pixel_width: u32::from(u16::MAX) + 1,
        ..size(80, 24)
    };
    let mut transport = LocalPtyTransport::launch(command([] as [&str; 0]));
    let (interactions, _requests) = interaction_channel();
    let error = transport
        .connect(&TransportRequest::new(overflow), interactions)
        .await
        .expect_err("pixel overflow must fail");
    assert_eq!(error.failure(), SessionFailure::Validation);

    assert_eq!(
        TransportRequest::new(size(80, 24))
            .with_terminal_type("")
            .expect_err("empty TERM")
            .failure(),
        SessionFailure::Validation
    );
    assert_eq!(
        TransportRequest::new(size(80, 24))
            .with_terminal_type("bad\0term")
            .expect_err("NUL TERM")
            .failure(),
        SessionFailure::Validation
    );
}

#[tokio::test]
async fn default_shell_accepts_input_and_exits_cleanly() {
    let mut transport = LocalPtyTransport::launch(LocalLaunch::DefaultShell);
    connect(&mut transport, &TransportRequest::new(size(80, 24))).await;
    transport
        .write(b"echo DEFAULT_SHELL_OK\r\nexit 0\r\n")
        .await
        .expect("write default-shell commands");
    let (output, status) = read_to_exit(&mut transport).await;
    assert!(contains(&output, b"DEFAULT_SHELL_OK"));
    assert!(status.success, "default shell exit was {status:?}");
    transport.shutdown().await.expect("shutdown default shell");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_hundred_cycles_reap_children_and_reader_threads() {
    tokio::time::timeout(Duration::from_secs(120), async {
        for cycle in 0..100 {
            let factory = LocalPtyFactory::new(command([] as [&str; 0]));
            let request = TransportRequest::new(size(80, 24));
            drop(factory.create(&request).expect("factory create"));
            let mut transport = LocalPtyTransport::launch(command([] as [&str; 0]));
            connect(&mut transport, &request).await;
            let pid = transport.process_id().unwrap_or_else(|| {
                panic!("cycle {cycle}: native child did not expose a process id")
            });
            transport.resize(size(81, 25)).await.expect("cycle resize");
            tokio::time::timeout(Duration::from_secs(2), transport.shutdown())
                .await
                .unwrap_or_else(|_| panic!("cycle {cycle}: shutdown timed out"))
                .expect("cycle shutdown");
            assert!(
                !process_is_active(pid),
                "cycle {cycle}: child {pid} is active"
            );
        }
    })
    .await
    .expect("100 PTY cleanup cycles timed out");
}

#[tokio::test]
async fn dropping_a_connected_transport_reaps_the_child() {
    let mut transport = LocalPtyTransport::launch(command([] as [&str; 0]));
    connect(&mut transport, &TransportRequest::new(size(80, 24))).await;
    let pid = transport.process_id().expect("native process id");
    drop(transport);
    assert!(!process_is_active(pid), "dropped child {pid} is active");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_is_bounded_when_a_descendant_inherits_the_pty() {
    let mut transport = LocalPtyTransport::launch(command(["--spawn-inheriting-child-ms", "3000"]));
    connect(&mut transport, &TransportRequest::new(size(80, 24))).await;
    let direct_pid = transport.process_id().expect("native process id");
    let output = read_until(&mut transport, b"DESCENDANT_READY").await;
    assert!(contains(&output, b"DESCENDANT:"));
    let descendant_pid = line_value(&output, b"DESCENDANT:");
    assert!(
        process_is_active(descendant_pid),
        "fixture descendant {descendant_pid} exited before shutdown"
    );

    let shutdown = tokio::spawn(async move { transport.shutdown().await });
    let result = tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .expect("shutdown must not wait for an inherited PTY descendant")
        .expect("shutdown task must not panic");
    if let Err(error) = result {
        assert_eq!(error.failure(), SessionFailure::Pty);
    }
    assert!(
        !process_is_active(direct_pid),
        "direct child {direct_pid} is active after bounded shutdown"
    );
    #[cfg(unix)]
    assert!(
        !process_is_active(descendant_pid),
        "fixture descendant {descendant_pid} is active after bounded shutdown"
    );
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_descendant_is_contained_before_first_user_marker() {
    for teardown in ["shutdown", "drop"] {
        let mut transport =
            LocalPtyTransport::launch(command(["--spawn-descendant-before-marker-ms", "30000"]));
        connect(&mut transport, &TransportRequest::new(size(80, 24))).await;
        let direct_pid = transport.process_id().expect("native process id");
        let output = read_until(&mut transport, b"FIRST_USER_MARKER").await;
        let descendant_pid = line_value(&output, b"DESCENDANT:");

        let app_inside = transport
            .process_tree_contains(std::process::id())
            .expect("query app Job membership");
        let direct_inside = transport
            .process_tree_contains(direct_pid)
            .expect("query direct-child Job membership");
        let descendant_inside = transport
            .process_tree_contains(descendant_pid)
            .expect("query descendant Job membership");
        let direct_alive_before = process_is_active(direct_pid);
        let descendant_alive_before = process_is_active(descendant_pid);
        assert!(
            !app_inside,
            "the rsHell test process must remain outside the per-session Job"
        );
        assert!(
            direct_inside,
            "direct child {direct_pid} was not in the Job before the first marker"
        );
        assert!(
            descendant_inside,
            "descendant {descendant_pid} was not in the Job before the first marker"
        );
        assert!(direct_alive_before);
        assert!(descendant_alive_before);

        if teardown == "shutdown" {
            tokio::time::timeout(Duration::from_secs(2), transport.shutdown())
                .await
                .expect("contained shutdown must be bounded")
                .expect("contained shutdown must converge");
        } else {
            drop(transport);
        }
        wait_until_process_stops(direct_pid).await;
        wait_until_process_stops(descendant_pid).await;
        eprintln!(
            "{teardown}: app_inside={app_inside} direct_pid={direct_pid} \
             direct_inside={direct_inside} direct_alive_before={direct_alive_before} \
             descendant_pid={descendant_pid} descendant_inside={descendant_inside} \
             descendant_alive_before={descendant_alive_before} both_dead_after=true"
        );
    }
}

#[cfg(windows)]
async fn wait_until_process_stops(pid: u32) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while process_is_active(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("process {pid} remained active after bounded teardown"));
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn line_value(output: &[u8], prefix: &[u8]) -> u32 {
    let start = output
        .windows(prefix.len())
        .position(|window| window == prefix)
        .map(|offset| offset + prefix.len())
        .unwrap_or_else(|| panic!("fixture line prefix missing from {output:?}"));
    let value = &output[start
        ..start
            + output[start..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count()];
    std::str::from_utf8(value)
        .expect("fixture value is UTF-8")
        .parse()
        .expect("fixture value is a process id")
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
