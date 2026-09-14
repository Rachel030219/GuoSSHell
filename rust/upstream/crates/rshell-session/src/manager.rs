use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};

use rshell_core::{SessionId, SessionState};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::{AbortHandle, JoinHandle, JoinSet},
};

const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

use crate::{
    SessionClient, SessionCommand, SessionError, SessionEvent, SessionLaunch, TransportFactory,
    actor::{ActorChannels, SessionActor},
    message::{COMMAND_CAPACITY, EVENT_CAPACITY, try_command},
};

const CRASH_MESSAGE: &str = "session actor crashed";

struct ActorEntry {
    commands: mpsc::Sender<SessionCommand>,
    actor_abort: AbortHandle,
    supervisor: JoinHandle<Result<(), SessionError>>,
}

type ActorRegistry = Arc<Mutex<BTreeMap<SessionId, ActorEntry>>>;
pub(crate) type ChildProcessRegistry = Arc<Mutex<BTreeMap<SessionId, u32>>>;

pub struct SessionManager {
    factory: Arc<dyn TransportFactory>,
    actors: ActorRegistry,
    child_processes: ChildProcessRegistry,
}

impl SessionManager {
    pub fn new(factory: Arc<dyn TransportFactory>) -> Self {
        Self {
            factory,
            actors: Arc::new(Mutex::new(BTreeMap::new())),
            child_processes: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn launch(&self, launch: SessionLaunch) -> Result<SessionClient, SessionError> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| SessionError::RuntimeUnavailable)?;
        let id = SessionId::new();
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = broadcast::channel(EVENT_CAPACITY);
        let (frame_tx, frame_rx) = watch::channel(None);
        let channels = ActorChannels {
            commands: command_rx,
            events: event_tx.clone(),
            frames: frame_tx,
        };
        let factory = launch.factory.unwrap_or_else(|| Arc::clone(&self.factory));
        let actor = SessionActor::new(
            id,
            factory,
            launch.request,
            launch.engine,
            launch.presentation_policy,
            channels,
            Arc::clone(&self.child_processes),
        );
        let (start_tx, start_rx) = oneshot::channel();
        let actor_task = runtime.spawn(async move {
            if start_rx.await.is_ok() {
                actor.run().await
            } else {
                Err(SessionError::ActorJoin)
            }
        });
        let actor_abort = actor_task.abort_handle();
        let actors = Arc::downgrade(&self.actors);
        let child_processes = Arc::clone(&self.child_processes);
        let supervisor = runtime.spawn(async move {
            let actor_result = match actor_task.await {
                Ok(result) => result,
                Err(error) => {
                    if error.is_panic() {
                        let _ = event_tx.send(SessionEvent::RecoveryChanged(None));
                        let _ = event_tx.send(SessionEvent::StateChanged(SessionState::Crashed));
                        let _ = event_tx.send(SessionEvent::Crashed(CRASH_MESSAGE.to_owned()));
                    }
                    Err(SessionError::ActorJoin)
                }
            };
            if let Some(actors) = actors.upgrade() {
                lock(&actors).remove(&id);
            }
            actor_result.and(verify_child_stopped(id, &child_processes))
        });
        lock(&self.actors).insert(
            id,
            ActorEntry {
                commands: command_tx.clone(),
                actor_abort,
                supervisor,
            },
        );
        if start_tx.send(()).is_err() {
            if let Some(actor) = lock(&self.actors).remove(&id) {
                actor.actor_abort.abort();
                actor.supervisor.abort();
            }
            return Err(SessionError::ActorJoin);
        }
        Ok(SessionClient {
            id,
            commands: command_tx,
            events: event_rx,
            frames: frame_rx,
        })
    }

    pub fn command(&self, id: SessionId, command: SessionCommand) -> Result<(), SessionError> {
        let actors = lock(&self.actors);
        let actor = actors.get(&id).ok_or(SessionError::UnknownSession)?;
        try_command(&actor.commands, command)
    }

    pub async fn shutdown(&self, id: SessionId) -> Result<(), SessionError> {
        let Some(actor) = lock(&self.actors).remove(&id) else {
            return Ok(());
        };
        stop_actor(actor).await
    }

    pub fn active_session_count(&self) -> usize {
        lock(&self.actors).len()
    }

    pub fn active_child_process_count(&self) -> usize {
        lock(&self.child_processes).len()
    }

    pub async fn shutdown_all(&self) -> Result<(), SessionError> {
        let actors = {
            let mut actors = lock(&self.actors);
            std::mem::take(&mut *actors)
        };
        let mut stopping = JoinSet::new();
        for (_, actor) in actors {
            stopping.spawn(stop_actor(actor));
        }
        let mut first_error = None;
        while let Some(result) = stopping.join_next().await {
            match result {
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                Ok(Ok(())) => {}
                Err(_) => {
                    first_error.get_or_insert(SessionError::ActorJoin);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

async fn stop_actor(mut actor: ActorEntry) -> Result<(), SessionError> {
    let sent_shutdown = matches!(
        tokio::time::timeout(
            SHUTDOWN_TIMEOUT,
            actor.commands.send(SessionCommand::Shutdown),
        )
        .await,
        Ok(Ok(()))
    );
    if !sent_shutdown {
        actor.actor_abort.abort();
    }

    match tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut actor.supervisor).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(SessionError::ActorJoin),
        Err(_) => {
            actor.actor_abort.abort();
            actor.supervisor.abort();
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut actor.supervisor).await;
            Err(SessionError::ActorJoin)
        }
    }
}

fn verify_child_stopped(
    id: SessionId,
    child_processes: &ChildProcessRegistry,
) -> Result<(), SessionError> {
    let mut processes = lock(child_processes);
    let Some(process_id) = processes.get(&id).copied() else {
        return Ok(());
    };
    if crate::process::is_active(process_id) {
        return Err(SessionError::ChildProcessAlive);
    }
    processes.remove(&id);
    Ok(())
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        let actors = {
            let mut actors = lock(&self.actors);
            std::mem::take(&mut *actors)
        };
        for (_, actor) in actors {
            actor.actor_abort.abort();
            actor.supervisor.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{future::pending, sync::Arc, time::Duration};

    use rshell_core::SessionFailure;

    use super::*;

    struct UnusedFactory;

    impl TransportFactory for UnusedFactory {
        fn create(
            &self,
            _request: &crate::TransportRequest,
        ) -> Result<Box<dyn crate::SessionTransport>, crate::TransportError> {
            Err(crate::TransportError::new(SessionFailure::Crashed))
        }
    }

    fn full_queue_hanging_entry() -> (ActorEntry, AbortHandle, AbortHandle) {
        let (commands, receiver) = mpsc::channel(1);
        commands
            .try_send(SessionCommand::Shutdown)
            .expect("fixture must fill the command queue");
        let actor = tokio::spawn(async move {
            let _receiver = receiver;
            pending::<()>().await;
        });
        let actor_abort = actor.abort_handle();
        let supervisor = tokio::spawn(pending::<Result<(), SessionError>>());
        let supervisor_abort = supervisor.abort_handle();
        (
            ActorEntry {
                commands,
                actor_abort: actor_abort.clone(),
                supervisor,
            },
            actor_abort,
            supervisor_abort,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_all_bounds_full_command_queues_and_drains_the_full_registry() {
        let manager = SessionManager::new(Arc::new(UnusedFactory));
        let (first, first_actor, first_supervisor) = full_queue_hanging_entry();
        let (second, second_actor, second_supervisor) = full_queue_hanging_entry();
        {
            let mut actors = lock(&manager.actors);
            actors.insert(SessionId::new(), first);
            actors.insert(SessionId::new(), second);
        }
        lock(&manager.child_processes).insert(SessionId::new(), std::process::id());

        let started = tokio::time::Instant::now();
        let shutdown = tokio::time::timeout(Duration::from_secs(5), manager.shutdown_all()).await;

        assert!(matches!(shutdown, Ok(Err(SessionError::ActorJoin))));
        assert_eq!(started.elapsed(), Duration::from_secs(4));
        assert_eq!(manager.active_session_count(), 0);
        assert!(first_actor.is_finished());
        assert!(second_actor.is_finished());
        assert!(first_supervisor.is_finished());
        assert!(second_supervisor.is_finished());
        assert_eq!(
            manager.active_child_process_count(),
            1,
            "a live transport PID must remain registered after failed teardown"
        );
    }

    #[test]
    fn empty_manager_reports_actor_and_real_transport_child_counts_separately() {
        let manager = SessionManager::new(Arc::new(UnusedFactory));
        assert_eq!(manager.active_session_count(), 0);
        assert_eq!(
            manager.active_child_process_count(),
            0,
            "transport child handles must not alias the actor registry"
        );
    }
}
