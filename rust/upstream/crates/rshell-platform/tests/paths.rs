use std::{fs, path::PathBuf};

use rshell_platform::{
    PlatformPaths, create_private_file, durable_replace_user_file, private_file_is_secure,
};

fn test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rshell-platform-{name}-{}", std::process::id()))
}

#[test]
fn roots_are_deterministic_and_ensure_exists_is_idempotent() {
    let root = test_root("paths");
    let paths =
        PlatformPaths::from_roots(root.join("config"), root.join("state"), root.join("cache"));

    paths.ensure_exists().unwrap();
    paths.ensure_exists().unwrap();

    assert!(paths.config_dir.is_dir());
    assert!(paths.state_dir.is_dir());
    assert!(paths.cache_dir.is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovered_paths_are_app_specific() {
    let paths = PlatformPaths::discover().unwrap();

    for directory in [&paths.config_dir, &paths.state_dir, &paths.cache_dir] {
        assert!(directory.components().any(|part| {
            part.as_os_str()
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("rshell")
        }));
    }
}

#[test]
fn known_hosts_is_app_owned_and_durable_replace_preserves_private_permissions() {
    let root = test_root("known-hosts");
    let paths =
        PlatformPaths::from_roots(root.join("config"), root.join("state"), root.join("cache"));
    paths.ensure_exists().unwrap();
    let destination = paths.known_hosts_path();
    assert_eq!(destination, paths.config_dir.join("known_hosts"));
    let source = paths.config_dir.join("known_hosts.tmp");
    let mut source_file = create_private_file(&source).unwrap();
    use std::io::Write;
    source_file.write_all(b"replacement").unwrap();
    drop(source_file);

    durable_replace_user_file(&source, &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!source.exists());
    assert!(private_file_is_secure(&destination).unwrap());
    fs::remove_dir_all(root).unwrap();
}
