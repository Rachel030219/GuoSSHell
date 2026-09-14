use std::{collections::BTreeMap, ffi::OsString, fmt, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use portable_pty::CommandBuilder;
use rshell_core::{SessionFailure, TerminalSize};
use rshell_platform::default_local_shell;

use crate::{
    InteractionBroker, SessionTransport, TransportCapabilities, TransportError, TransportEvent,
    TransportFactory, TransportRequest,
};

use super::{local_runtime::LocalRuntime, pty::spawn_pty_runtime};

#[derive(Clone)]
pub enum LocalLaunch {
    DefaultShell,
    Command {
        program: PathBuf,
        args: Vec<OsString>,
        cwd: Option<PathBuf>,
        env: BTreeMap<OsString, OsString>,
    },
}

impl fmt::Debug for LocalLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefaultShell => formatter.write_str("DefaultShell"),
            Self::Command { args, cwd, env, .. } => formatter
                .debug_struct("Command")
                .field("program", &"[REDACTED]")
                .field("argument_count", &args.len())
                .field("has_cwd", &cwd.is_some())
                .field("environment_count", &env.len())
                .finish(),
        }
    }
}

pub struct LocalPtyTransport {
    launch: LocalLaunch,
    runtime: Option<LocalRuntime>,
}

impl LocalPtyTransport {
    /// Creates a disconnected transport; the process is spawned by `connect`.
    pub fn launch(launch: LocalLaunch) -> Self {
        Self {
            launch,
            runtime: None,
        }
    }

    pub fn process_id(&self) -> Option<u32> {
        self.runtime.as_ref().and_then(LocalRuntime::process_id)
    }

    #[cfg(windows)]
    pub fn process_tree_contains(&self, process_id: u32) -> Result<bool, TransportError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| TransportError::new(SessionFailure::Pty))?
            .process_tree_contains(process_id)
    }

    fn command(&self, terminal_type: &str) -> Result<CommandBuilder, TransportError> {
        let (program, args, cwd, env) = match &self.launch {
            LocalLaunch::DefaultShell => {
                let shell = default_local_shell()
                    .map_err(|_| TransportError::new(SessionFailure::Platform))?;
                (shell.program, shell.args, None, shell.env)
            }
            LocalLaunch::Command {
                program,
                args,
                cwd,
                env,
            } => (program.clone(), args.clone(), cwd.clone(), env.clone()),
        };
        if program.as_os_str().is_empty() {
            return Err(TransportError::new(SessionFailure::Validation));
        }

        let mut command = CommandBuilder::new(program);
        command.args(args);
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        for (name, value) in env {
            command.env(name, value);
        }
        command.env("TERM", terminal_type);
        Ok(command)
    }

    fn runtime_mut(&mut self) -> Result<&mut LocalRuntime, TransportError> {
        self.runtime
            .as_mut()
            .ok_or_else(|| TransportError::new(SessionFailure::Pty))
    }
}

#[async_trait]
impl SessionTransport for LocalPtyTransport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities::default()
    }

    fn child_process_id(&self) -> Option<u32> {
        self.process_id()
    }

    async fn connect(
        &mut self,
        request: &TransportRequest,
        _interactions: InteractionBroker,
    ) -> Result<(), TransportError> {
        if self.runtime.is_some() {
            return Err(TransportError::new(SessionFailure::Validation));
        }
        let launch = self.launch.clone();
        let request = request.clone();
        let runtime = tokio::task::spawn_blocking(move || {
            let transport = Self::launch(launch);
            let command = transport.command(request.terminal_type())?;
            spawn_pty_runtime(command, request.initial_size(), SessionFailure::Pty)
        })
        .await
        .map_err(|_| TransportError::new(SessionFailure::Pty))??;
        self.runtime = Some(runtime);
        Ok(())
    }

    async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        self.runtime_mut()?.next_event().await
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.runtime_mut()?.write(bytes)
    }

    async fn resize(&mut self, size: TerminalSize) -> Result<(), TransportError> {
        self.runtime_mut()?.resize(size)
    }

    async fn shutdown(&mut self) -> Result<(), TransportError> {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.shutdown().await?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct LocalPtyFactory {
    launch: LocalLaunch,
}

impl LocalPtyFactory {
    pub fn new(launch: LocalLaunch) -> Arc<Self> {
        Arc::new(Self { launch })
    }
}

impl TransportFactory for LocalPtyFactory {
    fn create(
        &self,
        _request: &TransportRequest,
    ) -> Result<Box<dyn SessionTransport>, TransportError> {
        Ok(Box::new(LocalPtyTransport::launch(self.launch.clone())))
    }
}
