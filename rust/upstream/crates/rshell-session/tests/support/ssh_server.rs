use secrecy::{ExposeSecret, SecretString};
use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fmt,
    hash::{Hash, Hasher},
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use russh::{
    Channel, ChannelId, MethodSet,
    keys::{Algorithm, PrivateKey, PublicKey, decode_secret_key, key::safe_rng},
    server::{self, Auth, Msg, Response, Session},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpListener,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

pub const USERNAME: &str = "contract-user";
pub const PASSWORD: &str = "native-password-sentinel";
pub const KEY_PASSPHRASE: &str = "test";
pub const KBI_ANSWERS: [&str; 2] = ["user-visible", "one-time-code"];
pub const REMOTE_COMMAND_OUTPUT: &[u8] = b"remote-output-before-exit\r\n";

const ENCRYPTED_CLIENT_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABD1phlku5
A2G7Q9iP+DcOc9AAAAEAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIHeLC1lWiCYrXsf/
85O/pkbUFZ6OGIt49PX3nw8iRoXEAAAAkKRF0st5ZI7xxo9g6A4m4l6NarkQre3mycqNXQ
dP3jryYgvsCIBAA5jMWSjrmnOTXhidqcOy4xYCrAttzSnZ/cUadfBenL+DQq6neffw7j8r
0tbCxVGp6yCQlKrgSZf6c0Hy7dNEIU2bJFGxLe6/kWChcUAt/5Ll5rI7DVQPJdLgehLzvv
sJWR7W+cGvJ/vLsw==
-----END OPENSSH PRIVATE KEY-----";

#[derive(Clone)]
pub enum ServerAuth {
    Password,
    PasswordValue(String),
    PasswordFingerprint(u64),
    PublicKey(PublicKey),
    PublicKeys(Vec<PublicKey>),
    KeyboardInteractive,
}

pub fn password_fingerprint(secret: &SecretString) -> u64 {
    fingerprint(secret.expose_secret())
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ServerSnapshot {
    pub accepted_connections: usize,
    pub successful_authentications: usize,
    pub password_authentications: usize,
    pub public_key_authentications: usize,
    pub keyboard_interactive_authentications: usize,
    pub active_sessions: usize,
    pub open_channels: usize,
    pub opened_channels: usize,
    pub initial_pty: Option<(String, u32, u32, u32, u32)>,
    pub last_size: Option<(u32, u32, u32, u32)>,
    pub keyboard_answers: Vec<String>,
    pub received_input: Vec<u8>,
    pub remote_commands: Vec<Vec<u8>>,
    pub emitted_output_bytes: usize,
}

impl fmt::Debug for ServerSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerSnapshot")
            .field("accepted_connections", &self.accepted_connections)
            .field(
                "successful_authentications",
                &self.successful_authentications,
            )
            .field("password_authentications", &self.password_authentications)
            .field(
                "public_key_authentications",
                &self.public_key_authentications,
            )
            .field(
                "keyboard_interactive_authentications",
                &self.keyboard_interactive_authentications,
            )
            .field("active_sessions", &self.active_sessions)
            .field("open_channels", &self.open_channels)
            .field("opened_channels", &self.opened_channels)
            .field("initial_pty", &self.initial_pty)
            .field("last_size", &self.last_size)
            .field("keyboard_answers", &"[REDACTED]")
            .field(
                "received_input",
                &format_args!("[{} bytes]", self.received_input.len()),
            )
            .field(
                "remote_commands",
                &format_args!("[{} commands]", self.remote_commands.len()),
            )
            .field("emitted_output_bytes", &self.emitted_output_bytes)
            .finish()
    }
}

#[derive(Default)]
struct ProbeData {
    successful_authentications: usize,
    password_authentications: usize,
    public_key_authentications: usize,
    keyboard_interactive_authentications: usize,
    opened_channels: usize,
    initial_pty: Option<(String, u32, u32, u32, u32)>,
    last_size: Option<(u32, u32, u32, u32)>,
    keyboard_answers: Vec<String>,
    received_input: Vec<u8>,
    remote_commands: Vec<Vec<u8>>,
    emitted_output_bytes: usize,
}

#[derive(Default)]
struct ServerProbe {
    accepted_connections: AtomicUsize,
    active_sessions: AtomicUsize,
    open_channels: AtomicUsize,
    data: Mutex<ProbeData>,
}

impl ServerProbe {
    fn snapshot(&self) -> ServerSnapshot {
        let data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        ServerSnapshot {
            accepted_connections: self.accepted_connections.load(Ordering::SeqCst),
            successful_authentications: data.successful_authentications,
            password_authentications: data.password_authentications,
            public_key_authentications: data.public_key_authentications,
            keyboard_interactive_authentications: data.keyboard_interactive_authentications,
            active_sessions: self.active_sessions.load(Ordering::SeqCst),
            open_channels: self.open_channels.load(Ordering::SeqCst),
            opened_channels: data.opened_channels,
            initial_pty: data.initial_pty.clone(),
            last_size: data.last_size,
            keyboard_answers: data.keyboard_answers.clone(),
            received_input: data.received_input.clone(),
            remote_commands: data.remote_commands.clone(),
            emitted_output_bytes: data.emitted_output_bytes,
        }
    }
}

pub struct TestSshServer {
    address: SocketAddr,
    host_key: PublicKey,
    probe: Arc<ServerProbe>,
    exec_flush_probe: Option<Arc<ExecFlushProbe>>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), String>>>,
}

impl TestSshServer {
    pub async fn start(auth: ServerAuth) -> Self {
        Self::start_with_initial_output(auth, Vec::new()).await
    }

    pub async fn start_with_initial_output(auth: ServerAuth, initial_output: Vec<u8>) -> Self {
        Self::start_at_with_initial_output(
            "127.0.0.1:0".parse().unwrap(),
            auth,
            initial_output,
            None,
        )
        .await
    }

    pub async fn start_observing_exec_flushes(auth: ServerAuth) -> Self {
        Self::start_at_with_initial_output(
            "127.0.0.1:0".parse().unwrap(),
            auth,
            Vec::new(),
            Some(Arc::new(ExecFlushProbe::default())),
        )
        .await
    }

    pub async fn start_at(address: SocketAddr, auth: ServerAuth) -> Self {
        Self::start_at_with_initial_output(address, auth, Vec::new(), None).await
    }

    async fn start_at_with_initial_output(
        address: SocketAddr,
        auth: ServerAuth,
        initial_output: Vec<u8>,
        exec_flush_probe: Option<Arc<ExecFlushProbe>>,
    ) -> Self {
        let listener = TcpListener::bind(address)
            .await
            .expect("bind SSH test server");
        let address = listener.local_addr().expect("SSH test server address");
        let host_key =
            PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519).expect("generate SSH host key");
        let public_host_key = host_key.public_key().clone();
        let config = Arc::new(server::Config {
            methods: MethodSet::all(),
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            keys: vec![host_key],
            inactivity_timeout: Some(Duration::from_secs(10)),
            nodelay: true,
            ..Default::default()
        });
        let probe = Arc::new(ServerProbe::default());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(run_listener(
            listener,
            config,
            auth,
            initial_output,
            Arc::clone(&probe),
            exec_flush_probe.clone(),
            shutdown_rx,
        ));
        Self {
            address,
            host_key: public_host_key,
            probe,
            exec_flush_probe,
            shutdown: Some(shutdown_tx),
            task: Some(task),
        }
    }

    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn host_key(&self) -> &PublicKey {
        &self.host_key
    }

    pub fn snapshot(&self) -> ServerSnapshot {
        self.probe.snapshot()
    }

    pub fn exec_flush_generations(&self) -> usize {
        self.exec_flush_probe
            .as_ref()
            .expect("exec flush observation was not enabled")
            .completed_after_arm()
    }

    pub async fn shutdown(mut self) -> ServerSnapshot {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let task = self.task.take().expect("SSH server task");
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("SSH server leaked a session task")
            .expect("join SSH test server")
            .expect("SSH test server stopped cleanly");
        let snapshot = self.snapshot();
        assert_eq!(snapshot.active_sessions, 0, "SSH session leaked");
        assert_eq!(snapshot.open_channels, 0, "SSH channel leaked");
        snapshot
    }
}

impl Drop for TestSshServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub fn write_encrypted_client_key(directory: &Path) -> (PathBuf, PublicKey) {
    let path = directory.join("encrypted-client-key");
    let key = write_encrypted_client_key_to(&path);
    (path, key)
}

pub fn write_encrypted_client_key_to(path: &Path) -> PublicKey {
    std::fs::write(path, ENCRYPTED_CLIENT_KEY).expect("write encrypted client key");
    let key = decode_secret_key(ENCRYPTED_CLIENT_KEY, Some(KEY_PASSPHRASE))
        .expect("decode encrypted client key");
    key.public_key().clone()
}

pub async fn start_reset_server() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind reset server");
    let address = listener.local_addr().expect("reset server address");
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept reset client");
        drop(stream);
    });
    (address, task)
}

async fn run_listener(
    listener: TcpListener,
    config: Arc<server::Config>,
    auth: ServerAuth,
    initial_output: Vec<u8>,
    probe: Arc<ServerProbe>,
    exec_flush_probe: Option<Arc<ExecFlushProbe>>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(), String> {
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|_| "accept failed".to_owned())?;
                probe.accepted_connections.fetch_add(1, Ordering::SeqCst);
                probe.active_sessions.fetch_add(1, Ordering::SeqCst);
                let session_probe = Arc::clone(&probe);
                let handler = TestHandler::new(
                    auth.clone(),
                    initial_output.clone(),
                    Arc::clone(&probe),
                    exec_flush_probe.clone(),
                );
                let config = Arc::clone(&config);
                let stream = FlushRecordingStream::new(stream, exec_flush_probe.clone());
                sessions.spawn(async move {
                    let _active = ActiveSession(session_probe);
                    let running = server::run_stream(config, stream, handler).await;
                    if let Ok(running) = running {
                        let _ = running.await;
                    }
                });
            }
            Some(joined) = sessions.join_next(), if !sessions.is_empty() => {
                joined.map_err(|_| "SSH session task panicked".to_owned())?;
            }
        }
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(joined) = sessions.join_next().await {
            joined.map_err(|_| "SSH session task panicked".to_owned())?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "SSH session did not stop".to_owned())?
}

struct ActiveSession(Arc<ServerProbe>);

impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.0.active_sessions.fetch_sub(1, Ordering::SeqCst);
    }
}

struct TestHandler {
    auth: ServerAuth,
    initial_output: Vec<u8>,
    probe: Arc<ServerProbe>,
    exec_flush_probe: Option<Arc<ExecFlushProbe>>,
    channel_open: bool,
}

impl TestHandler {
    fn new(
        auth: ServerAuth,
        initial_output: Vec<u8>,
        probe: Arc<ServerProbe>,
        exec_flush_probe: Option<Arc<ExecFlushProbe>>,
    ) -> Self {
        Self {
            auth,
            initial_output,
            probe,
            exec_flush_probe,
            channel_open: false,
        }
    }

    fn record_success(&self, method: AuthenticationMethod) {
        let mut data = self
            .probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        data.successful_authentications += 1;
        match method {
            AuthenticationMethod::Password => data.password_authentications += 1,
            AuthenticationMethod::PublicKey => data.public_key_authentications += 1,
            AuthenticationMethod::KeyboardInteractive => {
                data.keyboard_interactive_authentications += 1
            }
        }
    }

    fn record_output(&self, bytes: usize) {
        self.probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .emitted_output_bytes += bytes;
    }

    fn accepts_public_key(&self, user: &str, public_key: &PublicKey) -> bool {
        if user != USERNAME {
            return false;
        }
        match &self.auth {
            ServerAuth::PublicKey(expected) => expected == public_key,
            ServerAuth::PublicKeys(expected) => expected.iter().any(|key| key == public_key),
            ServerAuth::Password
            | ServerAuth::PasswordValue(_)
            | ServerAuth::PasswordFingerprint(_)
            | ServerAuth::KeyboardInteractive => false,
        }
    }
}

impl server::Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let accepted = match &self.auth {
            ServerAuth::Password => password == PASSWORD,
            ServerAuth::PasswordValue(value) => password == value,
            ServerAuth::PasswordFingerprint(expected) => fingerprint(password) == *expected,
            ServerAuth::PublicKey(_)
            | ServerAuth::PublicKeys(_)
            | ServerAuth::KeyboardInteractive => false,
        };
        if user == USERNAME && accepted {
            self.record_success(AuthenticationMethod::Password);
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(if self.accepts_public_key(user, public_key) {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.accepts_public_key(user, public_key) {
            self.record_success(AuthenticationMethod::PublicKey);
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if !matches!(self.auth, ServerAuth::KeyboardInteractive) || user != USERNAME {
            return Ok(Auth::reject());
        }
        let Some(response) = response else {
            return Ok(Auth::Partial {
                name: Cow::Borrowed("Contract authentication"),
                instructions: Cow::Borrowed("Supply both answers in order"),
                prompts: Cow::Borrowed(&[
                    (Cow::Borrowed("Visible answer"), true),
                    (Cow::Borrowed("One-time code"), false),
                ]),
            });
        };
        let answers = response
            .map(|answer| String::from_utf8_lossy(&answer).into_owned())
            .collect::<Vec<_>>();
        self.probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keyboard_answers = answers.clone();
        if answers == KBI_ANSWERS {
            self.record_success(AuthenticationMethod::KeyboardInteractive);
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        self.channel_open = true;
        self.probe.open_channels.fetch_add(1, Ordering::SeqCst);
        self.probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .opened_channels += 1;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut data = self
            .probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        data.initial_pty = Some((
            term.to_owned(),
            col_width,
            row_height,
            pix_width,
            pix_height,
        ));
        data.last_size = Some((col_width, row_height, pix_width, pix_height));
        drop(data);
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"READY\r\n".as_slice())?;
        self.record_output(b"READY\r\n".len());
        if !self.initial_output.is_empty() {
            session.data(channel, self.initial_output.clone())?;
            self.record_output(self.initial_output.len());
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remote_commands
            .push(command.to_vec());
        session.channel_success(channel)?;
        session.data(channel, REMOTE_COMMAND_OUTPUT)?;
        self.record_output(REMOTE_COMMAND_OUTPUT.len());
        let status = command
            .strip_prefix(b"exit:")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        session.exit_status_request(channel, status)?;
        session.eof(channel)?;
        if let Some(probe) = &self.exec_flush_probe {
            probe.arm();
        }
        session
            .handle()
            .close(channel)
            .await
            .map_err(|()| russh::Error::SendError)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        bytes: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .received_input
            .extend_from_slice(bytes);
        session.data(channel, bytes.to_vec())?;
        self.record_output(bytes.len());
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.probe
            .data
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last_size = Some((col_width, row_height, pix_width, pix_height));
        let output = format!("RESIZED:{col_width}x{row_height}\r\n").into_bytes();
        session.data(channel, output.clone())?;
        self.record_output(output.len());
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.channel_open {
            self.channel_open = false;
            self.probe.open_channels.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[derive(Default)]
struct ExecFlushProbe {
    armed: AtomicBool,
    completed_flushes: AtomicUsize,
}

impl ExecFlushProbe {
    fn arm(&self) {
        assert!(
            !self.armed.swap(true, Ordering::SeqCst),
            "SSH test server supports one observed exec request per fixture"
        );
    }

    fn record_completed_flush(&self) {
        if self.armed.load(Ordering::SeqCst) {
            self.completed_flushes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn completed_after_arm(&self) -> usize {
        self.completed_flushes.load(Ordering::SeqCst)
    }
}

struct FlushRecordingStream {
    stream: tokio::net::TcpStream,
    probe: Option<Arc<ExecFlushProbe>>,
}

impl FlushRecordingStream {
    fn new(stream: tokio::net::TcpStream, probe: Option<Arc<ExecFlushProbe>>) -> Self {
        Self { stream, probe }
    }
}

impl AsyncRead for FlushRecordingStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for FlushRecordingStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.stream).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let result = Pin::new(&mut self.stream).poll_flush(context);
        if matches!(result, Poll::Ready(Ok(())))
            && let Some(probe) = &self.probe
        {
            probe.record_completed_flush();
        }
        result
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

fn fingerprint(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Copy)]
enum AuthenticationMethod {
    Password,
    PublicKey,
    KeyboardInteractive,
}

impl Drop for TestHandler {
    fn drop(&mut self) {
        if self.channel_open {
            self.probe.open_channels.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
