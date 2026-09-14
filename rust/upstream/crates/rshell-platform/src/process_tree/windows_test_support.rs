use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowsProcessJobTestFailure {
    Creation,
    Configuration,
}

#[derive(Clone, Debug)]
pub struct WindowsProcessJobTestHook {
    state: Arc<TestState>,
    failure: Option<WindowsProcessJobTestFailure>,
}

#[derive(Debug, Default)]
struct TestState {
    creation_calls: AtomicUsize,
    configuration_calls: AtomicUsize,
    termination_calls: AtomicUsize,
    closed_handles: AtomicUsize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsProcessJobTestSnapshot {
    pub creation_calls: usize,
    pub configuration_calls: usize,
    pub termination_calls: usize,
    pub closed_handles: usize,
}

impl WindowsProcessJobTestHook {
    pub fn observing() -> Self {
        Self::new(None)
    }

    pub fn failing(failure: WindowsProcessJobTestFailure) -> Self {
        Self::new(Some(failure))
    }

    fn new(failure: Option<WindowsProcessJobTestFailure>) -> Self {
        Self {
            state: Arc::new(TestState::default()),
            failure,
        }
    }

    pub fn snapshot(&self) -> WindowsProcessJobTestSnapshot {
        WindowsProcessJobTestSnapshot {
            creation_calls: self.state.creation_calls.load(Ordering::SeqCst),
            configuration_calls: self.state.configuration_calls.load(Ordering::SeqCst),
            termination_calls: self.state.termination_calls.load(Ordering::SeqCst),
            closed_handles: self.state.closed_handles.load(Ordering::SeqCst),
        }
    }

    pub(super) fn fails_at(&self, failure: WindowsProcessJobTestFailure) -> bool {
        self.failure == Some(failure)
    }

    pub(super) fn record_creation_call(&self) {
        self.state.creation_calls.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn record_configuration_call(&self) {
        self.state
            .configuration_calls
            .fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn record_termination_call(&self) {
        self.state.termination_calls.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn record_closed_handle(&self) {
        self.state.closed_handles.fetch_add(1, Ordering::SeqCst);
    }
}
