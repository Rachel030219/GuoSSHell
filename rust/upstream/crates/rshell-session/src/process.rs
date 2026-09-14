#[cfg(unix)]
pub(crate) fn is_active(process_id: u32) -> bool {
    unsafe extern "C" {
        fn kill(process_id: i32, signal: i32) -> i32;
    }
    i32::try_from(process_id).is_ok_and(|process_id| unsafe { kill(process_id, 0) } == 0)
}

#[cfg(windows)]
pub(crate) fn is_active(process_id: u32) -> bool {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return false;
    }
    let mut exit_code = 0;
    let active =
        unsafe { GetExitCodeProcess(process, &mut exit_code) } != 0 && exit_code == STILL_ACTIVE;
    unsafe {
        CloseHandle(process);
    }
    active
}
