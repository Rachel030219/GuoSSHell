use std::{sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{interval, timeout},
};

use super::ImportPortAdapter;

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum ImportCleanupError {
    #[error("import cleanup interval must be non-zero")]
    InvalidInterval,
    #[error("import cleanup task did not stop before its deadline")]
    Timeout,
    #[error("import cleanup task did not join cleanly")]
    Join,
}

pub struct ImportPreviewCleanup {
    cancel: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl ImportPreviewCleanup {
    pub fn start(adapter: &Arc<ImportPortAdapter>) -> Self {
        Self::start_with_interval(adapter, CLEANUP_INTERVAL)
            .expect("the fixed import cleanup interval is non-zero")
    }

    pub fn start_with_interval(
        adapter: &Arc<ImportPortAdapter>,
        interval_duration: Duration,
    ) -> Result<Self, ImportCleanupError> {
        if interval_duration.is_zero() {
            return Err(ImportCleanupError::InvalidInterval);
        }
        let adapter = Arc::downgrade(adapter);
        let (cancel, mut cancelled) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut ticks = interval(interval_duration);
            ticks.tick().await;
            loop {
                tokio::select! {
                    _ = ticks.tick() => {
                        let Some(adapter) = adapter.upgrade() else {
                            return;
                        };
                        adapter.cleanup_expired();
                    }
                    _ = &mut cancelled => return,
                }
            }
        });
        Ok(Self {
            cancel: Some(cancel),
            task,
        })
    }

    pub async fn shutdown(mut self) -> Result<(), ImportCleanupError> {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        match timeout(SHUTDOWN_TIMEOUT, &mut self.task).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(ImportCleanupError::Join),
            Err(_) => Err(ImportCleanupError::Timeout),
        }
    }
}

impl Drop for ImportPreviewCleanup {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        self.task.abort();
    }
}
