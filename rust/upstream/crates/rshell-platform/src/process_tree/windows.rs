use std::mem::size_of;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

#[cfg(feature = "containment-test-support")]
use super::windows_test_support::{WindowsProcessJobTestFailure, WindowsProcessJobTestHook};
use crate::PlatformError;

/// Owns one kill-on-close Windows Job used to contain a PTY process tree.
pub struct WindowsProcessJob {
    handle: Option<OwnedHandle>,
    #[cfg(feature = "containment-test-support")]
    test_hook: Option<WindowsProcessJobTestHook>,
}

impl WindowsProcessJob {
    pub fn new() -> Result<Self, PlatformError> {
        Self::new_inner(
            #[cfg(feature = "containment-test-support")]
            None,
        )
    }

    #[cfg(feature = "containment-test-support")]
    pub fn new_with_test_hook(hook: &WindowsProcessJobTestHook) -> Result<Self, PlatformError> {
        Self::new_inner(Some(hook.clone()))
    }

    fn new_inner(
        #[cfg(feature = "containment-test-support")] test_hook: Option<WindowsProcessJobTestHook>,
    ) -> Result<Self, PlatformError> {
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        #[cfg(feature = "containment-test-support")]
        if let Some(hook) = &test_hook {
            hook.record_creation_call();
        }
        if raw.is_null() {
            return Err(PlatformError::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
        #[cfg(feature = "containment-test-support")]
        if test_hook
            .as_ref()
            .is_some_and(|hook| hook.fails_at(WindowsProcessJobTestFailure::Creation))
        {
            drop(handle);
            test_hook
                .as_ref()
                .expect("creation failure hook is present")
                .record_closed_handle();
            return Err(PlatformError::injected_containment_failure());
        }
        let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle().cast(),
                JobObjectExtendedLimitInformation,
                (&raw const information).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        #[cfg(feature = "containment-test-support")]
        if let Some(hook) = &test_hook {
            hook.record_configuration_call();
        }
        #[cfg(feature = "containment-test-support")]
        let injected_configuration_failure = test_hook
            .as_ref()
            .is_some_and(|hook| hook.fails_at(WindowsProcessJobTestFailure::Configuration));
        #[cfg(not(feature = "containment-test-support"))]
        let injected_configuration_failure = false;
        if configured == 0 || injected_configuration_failure {
            let error = if configured == 0 {
                PlatformError::last_os_error()
            } else {
                #[cfg(feature = "containment-test-support")]
                {
                    PlatformError::injected_containment_failure()
                }
                #[cfg(not(feature = "containment-test-support"))]
                unreachable!()
            };
            drop(handle);
            #[cfg(feature = "containment-test-support")]
            if let Some(hook) = &test_hook {
                hook.record_closed_handle();
            }
            return Err(error);
        }
        Ok(Self {
            handle: Some(handle),
            #[cfg(feature = "containment-test-support")]
            test_hook,
        })
    }

    pub fn as_borrowed_handle(&self) -> BorrowedHandle<'_> {
        self.handle
            .as_ref()
            .expect("a terminated Job cannot be borrowed")
            .as_handle()
    }

    pub fn contains_process(&self, process_id: u32) -> Result<bool, PlatformError> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return Err(PlatformError::last_os_error());
        }
        let process = ProcessHandle(process);
        let mut contained = 0;
        let result = unsafe { IsProcessInJob(process.0, self.raw_handle()?, &mut contained) };
        if result == 0 {
            Err(PlatformError::last_os_error())
        } else {
            Ok(contained != 0)
        }
    }

    pub fn terminate(&mut self) -> Result<(), PlatformError> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        let result = unsafe { TerminateJobObject(handle.as_raw_handle().cast(), 1) };
        #[cfg(feature = "containment-test-support")]
        if let Some(hook) = &self.test_hook {
            hook.record_termination_call();
        }
        drop(handle);
        #[cfg(feature = "containment-test-support")]
        if let Some(hook) = &self.test_hook {
            hook.record_closed_handle();
        }
        if result == 0 {
            Err(PlatformError::last_os_error())
        } else {
            Ok(())
        }
    }

    fn raw_handle(&self) -> Result<HANDLE, PlatformError> {
        self.handle
            .as_ref()
            .map(|handle| handle.as_raw_handle().cast())
            .ok_or_else(PlatformError::last_os_error)
    }
}

impl Drop for WindowsProcessJob {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            drop(handle);
            #[cfg(feature = "containment-test-support")]
            if let Some(hook) = &self.test_hook {
                hook.record_closed_handle();
            }
        }
    }
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
