use std::{
    fs::File,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
    ptr,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        Authorization::{
            ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetEffectiveRightsFromAclW,
            SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
            TRUSTEE_IS_USER, TRUSTEE_W,
        },
        CopySid, DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation,
        InitializeSecurityDescriptor, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
        SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ,
        FILE_GENERIC_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
        ReplaceFileW,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::PlatformError;

// These standard access rights are omitted from the Win32 metadata used by `windows-sys`.
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_WRITE_BITS: u32 =
    FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES;
const CONTROL_WRITE_BITS: u32 = DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_WRITE;
const WRITE_BITS: u32 = FILE_WRITE_BITS | CONTROL_WRITE_BITS | FILE_GENERIC_WRITE;
const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;

pub(super) fn create_private_file(path: &Path) -> Result<File, PlatformError> {
    with_private_acl(|acl| {
        let mut descriptor: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
        let descriptor_ptr = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
        if unsafe { InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION) }
            == 0
        {
            return Err(PlatformError::last_os_error());
        }
        if unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, acl, 0) } == 0 {
            return Err(PlatformError::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor_ptr,
            bInheritHandle: 0,
        };
        let path = wide(path);
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(PlatformError::last_os_error());
        }
        Ok(unsafe { File::from_raw_handle(handle) })
    })
}

pub(super) fn harden_private_file(path: &Path) -> Result<(), PlatformError> {
    let path = wide(path);
    with_private_acl(|acl| set_dacl(path.as_ptr(), acl))
}

#[cfg(test)]
pub(super) fn make_insecure_for_test(path: &Path) -> Result<(), PlatformError> {
    let path = wide(path);
    let status = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr() as *mut u16,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    (status == 0).then_some(()).ok_or(PlatformError::Security)
}

pub(super) fn replace_file(source: &Path, destination: &Path) -> Result<(), PlatformError> {
    let source = wide(source);
    let destination = wide(destination);
    let replaced = if destination_exists(destination.as_ptr()) {
        unsafe {
            ReplaceFileW(
                destination.as_ptr(),
                source.as_ptr(),
                ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                ptr::null(),
                ptr::null(),
            )
        }
    } else {
        unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    (replaced != 0)
        .then_some(())
        .ok_or_else(PlatformError::last_os_error)
}

fn destination_exists(path: *const u16) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};

    unsafe { GetFileAttributesW(path) != INVALID_FILE_ATTRIBUTES }
}
pub(super) fn private_file_is_secure(path: &Path) -> Result<bool, PlatformError> {
    let path = wide(path);
    let mut dacl = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let status = unsafe {
        windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(PlatformError::Security);
    }
    let result = check_private_acl(dacl);
    unsafe { LocalFree(descriptor.cast()) };
    result
}

fn set_dacl(
    path: *const u16,
    acl: *mut windows_sys::Win32::Security::ACL,
) -> Result<(), PlatformError> {
    let status = unsafe {
        SetNamedSecurityInfoW(
            path as *mut u16,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        )
    };
    (status == 0).then_some(()).ok_or(PlatformError::Security)
}

fn with_private_acl<T>(
    operation: impl FnOnce(*mut windows_sys::Win32::Security::ACL) -> Result<T, PlatformError>,
) -> Result<T, PlatformError> {
    let user = current_user_sid()?;
    let system = Sid::from_string("S-1-5-18")?;
    let entries = [access_entry(user.as_ptr()), access_entry(system.as_ptr())];
    let mut acl = ptr::null_mut();
    let status = unsafe {
        SetEntriesInAclW(
            entries.len() as u32,
            entries.as_ptr(),
            ptr::null_mut(),
            &mut acl,
        )
    };
    if status != 0 {
        return Err(PlatformError::Security);
    }
    let result = operation(acl);
    unsafe { LocalFree(acl.cast()) };
    result
}

fn check_private_acl(acl: *mut windows_sys::Win32::Security::ACL) -> Result<bool, PlatformError> {
    if acl.is_null() {
        return Ok(false);
    }
    let user = current_user_sid()?;
    let system = Sid::from_string("S-1-5-18")?;
    let everyone = Sid::from_string("S-1-1-0")?;
    let builtin_users = Sid::from_string("S-1-5-32-545")?;
    Ok(can_write(acl, user.as_ptr())?
        && can_write(acl, system.as_ptr())?
        && !can_write(acl, everyone.as_ptr())?
        && !can_write(acl, builtin_users.as_ptr())?)
}

fn can_write(
    acl: *mut windows_sys::Win32::Security::ACL,
    sid: PSID,
) -> Result<bool, PlatformError> {
    let mut rights = 0;
    let trustee = trustee(sid);
    let status = unsafe { GetEffectiveRightsFromAclW(acl, &trustee, &mut rights) };
    (status == 0)
        .then_some(rights & WRITE_BITS != 0)
        .ok_or(PlatformError::Security)
}

fn access_entry(sid: PSID) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0,
        Trustee: trustee(sid),
    }
}

fn trustee(sid: PSID) -> TRUSTEE_W {
    TRUSTEE_W {
        pMultipleTrustee: ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        ptstrName: sid.cast(),
    }
}

fn current_user_sid() -> Result<Sid, PlatformError> {
    let mut token: HANDLE = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(PlatformError::last_os_error());
    }
    let result = token_user_sid(token);
    unsafe { CloseHandle(token) };
    result
}

fn token_user_sid(token: HANDLE) -> Result<Sid, PlatformError> {
    let mut bytes = 0;
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut bytes) };
    let mut buffer = vec![0_u8; bytes as usize];
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    } == 0
    {
        return Err(PlatformError::last_os_error());
    }
    let sid = unsafe { (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    Sid::copy(sid)
}

struct Sid(Vec<u8>);

impl Sid {
    fn copy(sid: PSID) -> Result<Self, PlatformError> {
        let mut bytes = vec![0_u8; unsafe { GetLengthSid(sid) } as usize];
        if unsafe { CopySid(bytes.len() as u32, bytes.as_mut_ptr().cast(), sid) } == 0 {
            return Err(PlatformError::last_os_error());
        }
        Ok(Self(bytes))
    }

    fn from_string(value: &str) -> Result<Self, PlatformError> {
        let mut raw = ptr::null_mut();
        let encoded: Vec<_> = value.encode_utf16().chain(Some(0)).collect();
        if unsafe { ConvertStringSidToSidW(encoded.as_ptr(), &mut raw) } == 0 {
            return Err(PlatformError::last_os_error());
        }
        let sid = Self::copy(raw);
        unsafe { LocalFree(raw.cast()) };
        sid
    }

    fn as_ptr(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
