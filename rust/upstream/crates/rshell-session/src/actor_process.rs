use rshell_core::SessionId;

use crate::{SessionError, manager::ChildProcessRegistry};

pub(crate) fn record_child_process(
    id: SessionId,
    registry: &ChildProcessRegistry,
    process_id: Option<u32>,
) {
    let mut processes = registry.lock().unwrap_or_else(|error| error.into_inner());
    match process_id {
        Some(process_id) => {
            processes.insert(id, process_id);
        }
        None => {
            processes.remove(&id);
        }
    }
}

pub(crate) fn clear_stopped_child_process(
    id: SessionId,
    registry: &ChildProcessRegistry,
) -> Result<(), SessionError> {
    let mut processes = registry.lock().unwrap_or_else(|error| error.into_inner());
    let Some(process_id) = processes.get(&id).copied() else {
        return Ok(());
    };
    if crate::process::is_active(process_id) {
        return Err(SessionError::ChildProcessAlive);
    }
    processes.remove(&id);
    Ok(())
}
