use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use rshell_platform::{create_private_file, durable_replace_user_file, private_file_is_secure};

#[cfg(any(unix, windows))]
use rshell_platform::harden_private_file;

#[cfg(windows)]
use std::{os::windows::ffi::OsStrExt, path::Path, ptr};

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, LocalFree},
    Security::{
        Authorization::{
            ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT,
            SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
        },
        CopySid, DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, PSID, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{FILE_GENERIC_READ, FILE_GENERIC_WRITE},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

// `WRITE_DAC` is omitted from the Win32 metadata used by `windows-sys`.
#[cfg(windows)]
const WRITE_DAC: u32 = 0x0004_0000;

fn test_file() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rshell-platform-private-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed),
    ))
}

#[test]
fn private_file_is_readable_writable_and_secure() {
    let path = test_file();
    let mut file = create_private_file(&path).unwrap();
    file.write_all(b"secret").unwrap();
    drop(file);

    let mut contents = String::new();
    fs::File::open(&path)
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert_eq!(contents, "secret");
    assert!(private_file_is_secure(&path).unwrap());
    fs::remove_file(path).unwrap();
}

#[test]
fn durable_replace_updates_an_existing_private_file_and_preserves_private_permissions() {
    let destination = test_file();
    let source = test_file();
    let mut old_file = create_private_file(&destination).unwrap();
    old_file.write_all(b"old").unwrap();
    drop(old_file);
    let mut replacement = create_private_file(&source).unwrap();
    replacement.write_all(b"replacement").unwrap();
    drop(replacement);

    durable_replace_user_file(&source, &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!source.exists());
    assert!(private_file_is_secure(&destination).unwrap());
    fs::remove_file(destination).unwrap();
}

#[cfg(unix)]
#[test]
fn harden_repairs_group_and_world_writable_file() {
    use std::os::unix::fs::PermissionsExt;

    let path = test_file();
    fs::write(&path, b"secret").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();

    assert!(!private_file_is_secure(&path).unwrap());
    harden_private_file(&path).unwrap();
    assert!(private_file_is_secure(&path).unwrap());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::remove_file(path).unwrap();
}

#[cfg(windows)]
#[test]
fn private_file_has_a_secure_dacl() {
    let path = test_file();
    let file = create_private_file(&path).unwrap();
    drop(file);

    assert!(private_file_is_secure(&path).unwrap());
    fs::remove_file(path).unwrap();
}

#[cfg(windows)]
#[test]
fn harden_repairs_inherited_and_dangerous_control_dacls() {
    let path = test_file();
    fs::write(&path, b"secret").unwrap();

    if !matches!(private_file_is_secure(&path), Ok(false)) {
        set_insecure_fixture_dacl(&path);
    }
    assert!(!private_file_is_secure(&path).unwrap());

    set_insecure_fixture_dacl(&path);
    assert!(!private_file_is_secure(&path).unwrap());

    harden_private_file(&path).unwrap();
    assert!(private_file_is_secure(&path).unwrap());
    fs::write(&path, b"hardened").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"hardened");
    fs::remove_file(path).unwrap();
}

#[cfg(windows)]
#[test]
fn durable_replace_hardens_an_insecure_existing_destination_before_and_after_replacement() {
    let destination = test_file();
    let source = test_file();
    fs::write(&destination, b"old").unwrap();
    set_insecure_fixture_dacl(&destination);
    assert!(!private_file_is_secure(&destination).unwrap());
    let mut replacement = create_private_file(&source).unwrap();
    replacement.write_all(b"replacement").unwrap();
    drop(replacement);

    durable_replace_user_file(&source, &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!source.exists());
    assert!(private_file_is_secure(&destination).unwrap());
    fs::remove_file(destination).unwrap();
}

#[cfg(windows)]
fn set_insecure_fixture_dacl(path: &Path) {
    let user = current_user_sid();
    let system = TestSid::from_string("S-1-5-18");
    let everyone = TestSid::from_string("S-1-1-0");
    let entries = [
        access_entry(user.as_ptr(), FILE_GENERIC_READ | FILE_GENERIC_WRITE),
        access_entry(system.as_ptr(), FILE_GENERIC_READ | FILE_GENERIC_WRITE),
        access_entry(everyone.as_ptr(), WRITE_DAC),
    ];
    let mut acl = ptr::null_mut();
    assert_eq!(
        unsafe {
            SetEntriesInAclW(
                entries.len() as u32,
                entries.as_ptr(),
                ptr::null_mut(),
                &mut acl,
            )
        },
        0
    );

    let path = wide(path);
    assert_eq!(
        unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr() as *mut u16,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | 0x8000_0000,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null_mut(),
            )
        },
        0
    );
    unsafe { LocalFree(acl.cast()) };
}

#[cfg(windows)]
fn access_entry(sid: PSID, permissions: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: permissions,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: sid.cast(),
        },
    }
}

#[cfg(windows)]
fn current_user_sid() -> TestSid {
    let mut token: HANDLE = ptr::null_mut();
    assert_ne!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
        0
    );
    let sid = token_user_sid(token);
    unsafe { CloseHandle(token) };
    sid
}

#[cfg(windows)]
fn token_user_sid(token: HANDLE) -> TestSid {
    let mut bytes = 0;
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut bytes) };
    let mut buffer = vec![0_u8; bytes as usize];
    assert_ne!(
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        },
        0
    );
    let sid = unsafe { (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    TestSid::copy(sid)
}

#[cfg(windows)]
struct TestSid(Vec<u8>);

#[cfg(windows)]
impl TestSid {
    fn copy(sid: PSID) -> Self {
        let mut bytes = vec![0_u8; unsafe { GetLengthSid(sid) } as usize];
        assert_ne!(
            unsafe { CopySid(bytes.len() as u32, bytes.as_mut_ptr().cast(), sid) },
            0
        );
        Self(bytes)
    }

    fn from_string(value: &str) -> Self {
        let mut raw = ptr::null_mut();
        let value: Vec<_> = value.encode_utf16().chain(Some(0)).collect();
        assert_ne!(
            unsafe { ConvertStringSidToSidW(value.as_ptr(), &mut raw) },
            0
        );
        let sid = Self::copy(raw);
        unsafe { LocalFree(raw.cast()) };
        sid
    }

    fn as_ptr(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
