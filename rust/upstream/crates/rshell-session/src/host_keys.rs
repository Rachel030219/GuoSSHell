mod error;
mod storage;

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rshell_core::{
    HostKeyDecision, HostKeyPrompt, InteractionId, InteractionRequest, InteractionResponse,
};
use rshell_platform::PlatformPaths;
use russh::keys::{HashAlg, PublicKey, known_hosts};
use tokio::sync::Mutex;

use crate::InteractionBroker;

pub use error::{HostKeyError, HostKeyStorageStep};

const DEFAULT_INTERACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Verifies SSH server keys against rsHell's application-owned known-hosts file.
///
/// Clones share the write lock, so an accepted prompt is always rechecked before it can mutate
/// the file. Waiting for a user response deliberately happens outside that lock.
#[derive(Clone)]
pub struct KnownHostsVerifier {
    path: PathBuf,
    timeout: Duration,
    write_lock: Arc<Mutex<()>>,
}

impl KnownHostsVerifier {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            timeout: DEFAULT_INTERACTION_TIMEOUT,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn for_platform(paths: &PlatformPaths) -> Self {
        Self::new(paths.known_hosts_path())
    }

    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn verify(
        &self,
        host: &str,
        port: u16,
        key: &PublicKey,
        interactions: &InteractionBroker,
    ) -> Result<(), HostKeyError> {
        if !valid_endpoint(host, port) {
            return Err(HostKeyError::InvalidEndpoint);
        }
        if self.is_known(host, port, key)? {
            return Ok(());
        }

        let response = self
            .request_confirmation(host, port, key, interactions)
            .await?;
        match response {
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore) => {
                self.store_after_acceptance(host, port, key).await
            }
            InteractionResponse::HostKey(HostKeyDecision::Reject)
            | InteractionResponse::Cancel
            | InteractionResponse::Secret(_)
            | InteractionResponse::Answers(_) => Err(HostKeyError::rejected(host, port)),
        }
    }

    async fn request_confirmation(
        &self,
        host: &str,
        port: u16,
        key: &PublicKey,
        interactions: &InteractionBroker,
    ) -> Result<InteractionResponse, HostKeyError> {
        let prompt = HostKeyPrompt {
            id: InteractionId::new(),
            host: host.to_owned(),
            port,
            algorithm: key.algorithm().to_string(),
            sha256: key.fingerprint(HashAlg::Sha256).to_string(),
            changed: false,
        };
        match tokio::time::timeout(
            self.timeout,
            interactions.request(InteractionRequest::HostKey(prompt)),
        )
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(HostKeyError::interaction(host, port)),
            Err(_) => Err(HostKeyError::timeout(host, port)),
        }
    }

    async fn store_after_acceptance(
        &self,
        host: &str,
        port: u16,
        key: &PublicKey,
    ) -> Result<(), HostKeyError> {
        let _write_guard = self.write_lock.lock().await;
        if self.is_known(host, port, key)? {
            return Ok(());
        }
        storage::store(&self.path, host, port, key)
    }

    fn is_known(&self, host: &str, port: u16, key: &PublicKey) -> Result<bool, HostKeyError> {
        if self.path.exists() && !self.path.is_file() {
            return Err(HostKeyError::verification(host, port));
        }
        match known_hosts::check_known_hosts_path(host, port, key, &self.path) {
            Ok(known) => Ok(known),
            Err(russh::keys::Error::KeyChanged { line }) => {
                Err(HostKeyError::changed(host, port, line))
            }
            Err(_) => Err(HostKeyError::verification(host, port)),
        }
    }
}

impl fmt::Debug for KnownHostsVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KnownHostsVerifier([REDACTED])")
    }
}

fn valid_endpoint(host: &str, _port: u16) -> bool {
    !host.is_empty() && !host.chars().any(char::is_whitespace)
}
