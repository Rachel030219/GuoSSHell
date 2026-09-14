#[cfg(windows)]
mod windows;
#[cfg(all(windows, feature = "containment-test-support"))]
mod windows_test_support;

#[cfg(windows)]
pub use windows::WindowsProcessJob;
#[cfg(all(windows, feature = "containment-test-support"))]
pub use windows_test_support::{
    WindowsProcessJobTestFailure, WindowsProcessJobTestHook, WindowsProcessJobTestSnapshot,
};
