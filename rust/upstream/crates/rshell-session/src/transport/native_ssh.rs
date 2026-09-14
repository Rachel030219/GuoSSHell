mod auth;
mod channel;
mod config;
mod error;
mod handler;

use std::{collections::VecDeque, future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use rshell_core::{ConnectionProfile, SessionFailure, TerminalSize};
use russh::{Channel, client};
use tokio::net::TcpStream;

use crate::{
    AuthPlan, InteractionBroker, KnownHostsVerifier, SessionTransport, TransportCapabilities,
    TransportError, TransportEvent, TransportRequest,
};

use self::{
    channel::{channel_event, configure_channel},
    config::{validate_profile, validate_request, validate_size},
    handler::StrictClientHandler,
};

const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);

/// A managed SSH transport backed exclusively by russh.
pub struct NativeSshTransport {
    profile: ConnectionProfile,
    auth: Option<AuthPlan>,
    verifier: KnownHostsVerifier,
    timeout: Duration,
    handle: Option<client::Handle<StrictClientHandler>>,
    channel: Option<Channel<client::Msg>>,
    pending_events: VecDeque<TransportEvent>,
    shutdown: bool,
}

impl NativeSshTransport {
    pub fn new(
        profile: ConnectionProfile,
        auth: AuthPlan,
        verifier: KnownHostsVerifier,
    ) -> Result<Self, TransportError> {
        validate_profile(&profile, &auth)?;
        Ok(Self {
            profile,
            auth: Some(auth),
            verifier,
            timeout: DEFAULT_OPERATION_TIMEOUT,
            handle: None,
            channel: None,
            pending_events: VecDeque::new(),
            shutdown: false,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, TransportError> {
        if timeout.is_zero() {
            return Err(TransportError::new(SessionFailure::Validation));
        }
        self.timeout = timeout;
        Ok(self)
    }

    async fn connect_inner(
        &mut self,
        request: &TransportRequest,
        interactions: InteractionBroker,
    ) -> Result<(), TransportError> {
        if self.handle.is_some() || self.channel.is_some() || self.shutdown {
            return Err(TransportError::new(SessionFailure::Validation));
        }
        validate_request(request)?;
        if self.auth.is_none() {
            return Err(TransportError::new(SessionFailure::Authentication));
        }
        let target = (self.profile.host.as_str(), self.profile.port);
        let stream = TcpStream::connect(target)
            .await
            .map_err(|_| TransportError::new(SessionFailure::Network))?;
        stream
            .set_nodelay(true)
            .map_err(|_| TransportError::new(SessionFailure::Network))?;
        let peer = stream
            .peer_addr()
            .map_err(|_| TransportError::new(SessionFailure::Network))?;
        let handler = StrictClientHandler::new(
            peer.ip().to_string(),
            peer.port(),
            self.verifier.clone(),
            interactions.clone(),
        );
        let config = Arc::new(client::Config {
            nodelay: true,
            ..Default::default()
        });
        let mut handle = client::connect_stream(config, stream, handler)
            .await
            .map_err(TransportError::from)?;
        let Some(auth) = self.auth.take() else {
            disconnect_quietly(&handle).await;
            return Err(TransportError::new(SessionFailure::Authentication));
        };
        if let Err(error) =
            auth::authenticate(&mut handle, &self.profile.username, auth, &interactions).await
        {
            disconnect_quietly(&handle).await;
            return Err(error);
        }

        let mut channel = match handle.channel_open_session().await {
            Ok(channel) => channel,
            Err(_) => {
                disconnect_quietly(&handle).await;
                return Err(TransportError::new(SessionFailure::SshChannel));
            }
        };
        if configure_channel(
            &mut channel,
            request,
            self.profile.remote_command.as_deref(),
            &mut self.pending_events,
        )
        .await
        .is_err()
        {
            let _ = channel.eof().await;
            let _ = channel.close().await;
            disconnect_quietly(&handle).await;
            return Err(TransportError::new(SessionFailure::SshChannel));
        }
        self.channel = Some(channel);
        self.handle = Some(handle);
        Ok(())
    }

    fn channel_mut(&mut self) -> Result<&mut Channel<client::Msg>, TransportError> {
        self.channel
            .as_mut()
            .ok_or_else(|| TransportError::new(SessionFailure::SshChannel))
    }
}

#[async_trait]
impl SessionTransport for NativeSshTransport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            agent: false,
            public_key: true,
            managed_password: true,
            keyboard_interactive: true,
            host_key_prompt: true,
        }
    }

    async fn connect(
        &mut self,
        request: &TransportRequest,
        interactions: InteractionBroker,
    ) -> Result<(), TransportError> {
        bounded(self.timeout, self.connect_inner(request, interactions)).await
    }

    async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        if let Some(event) = self.pending_events.pop_front() {
            return Ok(event);
        }
        loop {
            let message = self.channel_mut()?.wait().await;
            if let Some(event) = channel_event(message) {
                return Ok(event);
            }
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let timeout = self.timeout;
        bounded(timeout, async {
            self.channel_mut()?
                .data_bytes(bytes.to_vec())
                .await
                .map_err(|_| TransportError::new(SessionFailure::SshChannel))
        })
        .await
    }

    async fn resize(&mut self, size: TerminalSize) -> Result<(), TransportError> {
        validate_size(size)?;
        let timeout = self.timeout;
        bounded(timeout, async {
            self.channel_mut()?
                .window_change(
                    u32::from(size.cols),
                    u32::from(size.rows),
                    size.pixel_width,
                    size.pixel_height,
                )
                .await
                .map_err(|_| TransportError::new(SessionFailure::SshChannel))
        })
        .await
    }

    async fn shutdown(&mut self) -> Result<(), TransportError> {
        if self.shutdown {
            return Ok(());
        }
        self.shutdown = true;
        let timeout = self.timeout;
        bounded(timeout, async {
            if let Some(channel) = self.channel.take() {
                let _ = channel.eof().await;
                let _ = channel.close().await;
            }
            if let Some(handle) = self.handle.take() {
                disconnect_quietly(&handle).await;
            }
            Ok(())
        })
        .await
    }
}

async fn bounded<T>(
    timeout: Duration,
    future: impl Future<Output = Result<T, TransportError>>,
) -> Result<T, TransportError> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| TransportError::new(SessionFailure::Timeout))?
}

async fn disconnect_quietly(handle: &client::Handle<StrictClientHandler>) {
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "")
        .await;
}
