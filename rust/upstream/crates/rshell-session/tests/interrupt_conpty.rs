#![cfg(windows)]

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use rshell_core::{PaneId, SessionPort, SessionUiCommand, TerminalOverrides, TerminalProfile};
use rshell_session::{
    InteractionBroker, KnownHostsVerifier, LocalLaunch, LocalPtyTransport, SessionManager,
    SessionTransport, TransportCapabilities, TransportError, TransportEvent, TransportFactory,
    TransportRequest, ports::SessionPortAdapter,
};
use tokio::sync::watch;

const WAIT: Duration = Duration::from_secs(10);
const CLEANUP_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct RecordingLocalFactory {
    launch: LocalLaunch,
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl TransportFactory for RecordingLocalFactory {
    fn create(
        &self,
        _request: &TransportRequest,
    ) -> Result<Box<dyn SessionTransport>, TransportError> {
        Ok(Box::new(RecordingLocalTransport {
            inner: LocalPtyTransport::launch(self.launch.clone()),
            writes: Arc::clone(&self.writes),
        }))
    }
}

struct RecordingLocalTransport {
    inner: LocalPtyTransport,
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[async_trait]
impl SessionTransport for RecordingLocalTransport {
    fn capabilities(&self) -> TransportCapabilities {
        self.inner.capabilities()
    }

    fn child_process_id(&self) -> Option<u32> {
        self.inner.child_process_id()
    }

    async fn connect(
        &mut self,
        request: &TransportRequest,
        interactions: InteractionBroker,
    ) -> Result<(), TransportError> {
        self.inner.connect(request, interactions).await
    }

    async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        self.inner.next_event().await
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.inner.write(bytes).await?;
        lock(&self.writes).push(bytes.to_vec());
        Ok(())
    }

    async fn resize(&mut self, size: rshell_core::TerminalSize) -> Result<(), TransportError> {
        self.inner.resize(size).await
    }

    async fn shutdown(&mut self) -> Result<(), TransportError> {
        self.inner.shutdown().await
    }
}

#[ignore = "requires a real Windows ConPTY session"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_reaches_direct_conpty_child_as_single_etx() {
    run_interrupt_case(LocalLaunch::Command {
        program: fixture(),
        args: vec![OsString::from("survive")],
        cwd: None,
        env: BTreeMap::new(),
    })
    .await;
}

#[ignore = "requires a real Windows ConPTY session"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_reaches_child_of_pwsh_conpty_session_as_single_etx() {
    let fixture = fixture();
    run_interrupt_case(LocalLaunch::Command {
        program: PathBuf::from("pwsh.exe"),
        args: vec![
            OsString::from("-NoProfile"),
            OsString::from("-Command"),
            OsString::from(format!(
                "& '{}' survive",
                escape_powershell_single_quoted_path(&fixture)
            )),
        ],
        cwd: None,
        env: BTreeMap::new(),
    })
    .await;
}

async fn run_interrupt_case(launch: LocalLaunch) {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let manager = Arc::new(SessionManager::new(Arc::new(RecordingLocalFactory {
        launch,
        writes: Arc::clone(&writes),
    })));
    let known_hosts = tempfile::tempdir().expect("create known-hosts directory");
    let adapter = SessionPortAdapter::new(
        Arc::clone(&manager),
        KnownHostsVerifier::new(known_hosts.path().join("known_hosts")),
    );
    let mut settings = TerminalProfile::default().settings;
    settings.enable_csi_u = true;
    settings.enable_kitty_keyboard = true;
    let terminal = settings.resolve(&TerminalOverrides::default());
    let mut binding = tokio::time::timeout(WAIT, adapter.launch_local(PaneId::new(), terminal))
        .await
        .expect("local session launch timed out")
        .expect("local session launch");

    wait_for_frame(
        &mut binding.frames,
        |frame| {
            frame.display_modes.alternate_screen
                && frame.display_modes.enhanced_keyboard
                && frame_contains(frame, "fixture-界-e")
        },
        "fixture startup frame",
    )
    .await;

    tokio::time::timeout(
        WAIT,
        adapter.command(binding.id, SessionUiCommand::Interrupt),
    )
    .await
    .expect("interrupt command timed out")
    .expect("interrupt command");
    wait_for_frame(
        &mut binding.frames,
        |frame| frame_contains(frame, "interrupt=03;survived=true"),
        "fixture interrupt observation",
    )
    .await;

    let writes = lock(&writes).clone();
    assert_eq!(
        writes,
        vec![vec![0x03]],
        "interrupt must write exactly one ETX through the actor"
    );
    assert!(
        !writes.iter().any(|bytes| is_csi_u(bytes)),
        "interrupt must not emit a CSI-u sequence: {writes:?}"
    );

    tokio::time::timeout(CLEANUP_WAIT, adapter.shutdown(binding.id))
        .await
        .expect("session cleanup timed out")
        .expect("session cleanup");
    assert_eq!(manager.active_session_count(), 0);
    assert_eq!(manager.active_child_process_count(), 0);
}

async fn wait_for_frame(
    frames: &mut watch::Receiver<Option<Arc<rshell_core::RenderFrame>>>,
    predicate: impl Fn(&rshell_core::RenderFrame) -> bool,
    expectation: &str,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(frame) = frames.borrow_and_update().clone()
                && predicate(&frame)
            {
                return;
            }
            frames.changed().await.expect("frame channel closed");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {expectation}"));
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rshell-interrupt-tui"))
}

fn escape_powershell_single_quoted_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn frame_contains(frame: &rshell_core::RenderFrame, expected: &str) -> bool {
    frame.rows.iter().any(|row| {
        row.cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains(expected)
    })
}

fn is_csi_u(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x1b[") && bytes.ends_with(b"u")
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
