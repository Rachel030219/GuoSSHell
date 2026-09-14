use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use async_trait::async_trait;
use portable_pty::CommandBuilder;
use rshell_core::{ConnectionProfile, SessionFailure, TerminalSize};
use rshell_platform::ssh_executable;

use crate::{
    InteractionBroker, SessionTransport, TransportCapabilities, TransportError, TransportEvent,
    TransportRequest,
};

use super::{local_runtime::LocalRuntime, pty::spawn_pty_runtime};

/// Builds the system OpenSSH argument vector without using a command-line parser.
pub fn build_system_ssh_argv(profile: &ConnectionProfile) -> Result<Vec<OsString>, TransportError> {
    validate_profile(profile)?;

    let mut args = Vec::new();
    if profile.port != 22 {
        args.push(OsString::from("-p"));
        args.push(OsString::from(profile.port.to_string()));
    }
    if let Some(identity_file) = &profile.identity_file {
        args.push(OsString::from("-i"));
        args.push(identity_file.as_os_str().to_os_string());
        args.push(OsString::from("-o"));
        args.push(OsString::from("IdentitiesOnly=yes"));
    }
    args.push(OsString::from("-o"));
    args.push(OsString::from("StrictHostKeyChecking=ask"));
    args.push(OsString::from("--"));
    args.push(OsString::from(destination(profile)));
    if let Some(remote_command) = &profile.remote_command {
        args.push(OsString::from(remote_command));
    }
    Ok(args)
}

/// A PTY-backed system OpenSSH client which relies on the user's SSH agent and configuration.
pub struct SystemOpenSshTransport {
    profile: ConnectionProfile,
    runtime: Option<LocalRuntime>,
}

impl SystemOpenSshTransport {
    /// Creates a disconnected system OpenSSH transport.
    pub fn new(profile: ConnectionProfile) -> Self {
        Self {
            profile,
            runtime: None,
        }
    }

    pub fn process_id(&self) -> Option<u32> {
        self.runtime.as_ref().and_then(LocalRuntime::process_id)
    }

    fn connect_inner(&mut self, request: &TransportRequest) -> Result<(), TransportError> {
        if self.runtime.is_some() {
            return Err(validation_error());
        }
        let args = build_system_ssh_argv(&self.profile)?;
        let program =
            ssh_executable().map_err(|_| TransportError::new(SessionFailure::Platform))?;
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.env("TERM", request.terminal_type());
        self.runtime = Some(spawn_pty_runtime(
            command,
            request.initial_size(),
            SessionFailure::Subprocess,
        )?);
        Ok(())
    }

    fn runtime_mut(&mut self) -> Result<&mut LocalRuntime, TransportError> {
        self.runtime
            .as_mut()
            .ok_or_else(|| TransportError::new(SessionFailure::Pty))
    }
}

#[async_trait]
impl SessionTransport for SystemOpenSshTransport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            agent: true,
            public_key: true,
            managed_password: false,
            keyboard_interactive: false,
            host_key_prompt: true,
        }
    }

    fn child_process_id(&self) -> Option<u32> {
        self.process_id()
    }

    async fn connect(
        &mut self,
        request: &TransportRequest,
        _interactions: InteractionBroker,
    ) -> Result<(), TransportError> {
        self.connect_inner(request)
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

fn validate_profile(profile: &ConnectionProfile) -> Result<(), TransportError> {
    if profile.port == 0
        || invalid_text(&profile.host)
        || profile.host.starts_with('-')
        || profile.host.contains('@')
        || invalid_optional_text(&profile.username)
        || profile.username.starts_with('-')
        || profile.username.contains('@')
        || profile
            .identity_file
            .as_deref()
            .is_some_and(invalid_identity_file)
        || profile
            .remote_command
            .as_deref()
            .is_some_and(invalid_remote_command)
    {
        return Err(validation_error());
    }
    Ok(())
}

fn destination(profile: &ConnectionProfile) -> String {
    if profile.username.is_empty() {
        profile.host.clone()
    } else {
        format!("{}@{}", profile.username, profile.host)
    }
}

fn invalid_text(value: &str) -> bool {
    value.is_empty() || value.contains(['\0', '\r', '\n'])
}

fn invalid_optional_text(value: &str) -> bool {
    value.contains(['\0', '\r', '\n'])
}

fn invalid_identity_file(path: &Path) -> bool {
    let value: &OsStr = path.as_os_str();
    value.is_empty()
        || value
            .as_encoded_bytes()
            .iter()
            .any(|byte| matches!(byte, b'\0' | b'\r' | b'\n'))
        || value.as_encoded_bytes().starts_with(b"-")
}

fn invalid_remote_command(command: &str) -> bool {
    command.trim().is_empty() || command.contains(['\0', '\r', '\n'])
}

fn validation_error() -> TransportError {
    TransportError::new(SessionFailure::Validation)
}
