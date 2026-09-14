use rshell_core::SessionFailure;
use russh::{client, keys::PublicKey};

use crate::{InteractionBroker, KnownHostsVerifier};

use super::error::NativeClientError;

pub(super) struct StrictClientHandler {
    host: String,
    port: u16,
    verifier: KnownHostsVerifier,
    interactions: InteractionBroker,
}

impl StrictClientHandler {
    pub(super) fn new(
        host: String,
        port: u16,
        verifier: KnownHostsVerifier,
        interactions: InteractionBroker,
    ) -> Self {
        Self {
            host,
            port,
            verifier,
            interactions,
        }
    }
}

impl client::Handler for StrictClientHandler {
    type Error = NativeClientError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        self.verifier
            .verify(&self.host, self.port, server_public_key, &self.interactions)
            .await
            .map_err(|error| NativeClientError::new(error.failure()))?;
        Ok(true)
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            client::DisconnectReason::Error(error)
                if matches!(
                    error.failure(),
                    SessionFailure::Network | SessionFailure::Timeout
                ) =>
            {
                Err(error)
            }
            client::DisconnectReason::Error(error) => Err(error),
        }
    }
}
