use crate::win::psuedocon::HPCON;
use anyhow::{ensure, Error};
use std::io::Error as IoError;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::{mem, ptr};
use winapi::shared::minwindef::DWORD;
use winapi::um::processthreadsapi::*;
use winapi::um::winnt::HANDLE;

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x00020016;
const PROC_THREAD_ATTRIBUTE_JOB_LIST: usize = 0x0002000D;

pub struct ProcThreadAttributeList {
    data: Vec<u8>,
    job_handles: Option<Box<[HANDLE; 1]>>,
    #[cfg(feature = "containment-test-support")]
    test_hook: Option<crate::ContainmentTestHook>,
}

impl ProcThreadAttributeList {
    pub fn with_capacity(num_attributes: DWORD) -> Result<Self, Error> {
        let mut bytes_required: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(
                ptr::null_mut(),
                num_attributes,
                0,
                &mut bytes_required,
            )
        };
        let mut data = Vec::with_capacity(bytes_required);
        // We have the right capacity, so force the vec to consider itself
        // that length.  The contents of those bytes will be maintained
        // by the win32 apis used in this impl.
        unsafe { data.set_len(bytes_required) };

        let attr_ptr = data.as_mut_slice().as_mut_ptr() as *mut _;
        let res = unsafe {
            InitializeProcThreadAttributeList(attr_ptr, num_attributes, 0, &mut bytes_required)
        };
        ensure!(
            res != 0,
            "InitializeProcThreadAttributeList failed: {}",
            IoError::last_os_error()
        );
        Ok(Self {
            data,
            job_handles: None,
            #[cfg(feature = "containment-test-support")]
            test_hook: None,
        })
    }

    pub fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.data.as_mut_slice().as_mut_ptr() as *mut _
    }

    pub fn set_pty(&mut self, con: HPCON) -> Result<(), Error> {
        let res = unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                con,
                mem::size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        ensure!(
            res != 0,
            "UpdateProcThreadAttribute failed: {}",
            IoError::last_os_error()
        );
        Ok(())
    }

    pub fn set_job(&mut self, job: BorrowedHandle<'_>) -> Result<(), Error> {
        self.set_job_inner(
            job,
            #[cfg(feature = "containment-test-support")]
            None,
        )
    }

    #[cfg(feature = "containment-test-support")]
    pub fn set_job_with_test_hook(
        &mut self,
        job: BorrowedHandle<'_>,
        hook: &crate::ContainmentTestHook,
    ) -> Result<(), Error> {
        self.test_hook = Some(hook.clone());
        self.set_job_inner(job, Some(hook))
    }

    fn set_job_inner(
        &mut self,
        job: BorrowedHandle<'_>,
        #[cfg(feature = "containment-test-support")] test_hook: Option<&crate::ContainmentTestHook>,
    ) -> Result<(), Error> {
        ensure!(self.job_handles.is_none(), "JOB_LIST already configured");
        self.job_handles = Some(Box::new([job.as_raw_handle() as HANDLE]));
        let attribute_list = self.as_mut_ptr();
        let handles = self.job_handles.as_mut().expect("job storage was just initialized");
        let job_list_value = handles.as_mut_ptr().cast();
        let res = unsafe {
            UpdateProcThreadAttribute(
                attribute_list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST,
                job_list_value,
                mem::size_of::<HANDLE>(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        #[cfg(feature = "containment-test-support")]
        let res = if let Some(hook) = test_hook {
            hook.record_job_attribute_update();
            if res != 0 && hook.fail_job_attribute_update() {
                0
            } else {
                res
            }
        } else {
            res
        };
        ensure!(
            res != 0,
            "UpdateProcThreadAttribute JOB_LIST failed: {}",
            IoError::last_os_error()
        );
        Ok(())
    }
}

impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
        #[cfg(feature = "containment-test-support")]
        if let Some(hook) = &self.test_hook {
            hook.record_attribute_list_destroyed();
        }
        // `job_handles` remains live until after attribute-list destruction.
    }
}
