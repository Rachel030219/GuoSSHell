use std::fmt;

use rshell_core::CredentialRef;
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroize;

pub use crate::memory_vault::{
    MemoryCredentialVault, MemoryVaultCallCounts, MemoryVaultFault, VaultMutation, VaultOperation,
};

pub const SYSTEM_CREDENTIAL_SERVICE: &str = "io.github.hugefiver.rshell";

pub trait CredentialVault: Send + Sync {
    fn get(&self, credential_ref: &CredentialRef) -> Result<Option<SecretString>, VaultError>;
    fn put(&self, credential_ref: &CredentialRef, value: &SecretString) -> Result<(), VaultError>;
    fn delete(&self, credential_ref: &CredentialRef) -> Result<(), VaultError>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    Unavailable,
    NoEntry,
    Denied,
    Platform,
}

impl VaultError {
    const fn category(self) -> &'static str {
        match self {
            Self::Unavailable => "Unavailable",
            Self::NoEntry => "NoEntry",
            Self::Denied => "Denied",
            Self::Platform => "Platform",
        }
    }
}

impl fmt::Debug for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category())
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category())
    }
}

impl std::error::Error for VaultError {}

#[derive(Default)]
pub struct SystemCredentialVault;

impl SystemCredentialVault {
    pub const fn new() -> Self {
        Self
    }

    fn entry(credential_ref: &CredentialRef) -> Result<keyring::Entry, VaultError> {
        keyring::Entry::new(SYSTEM_CREDENTIAL_SERVICE, &credential_ref.0).map_err(map_keyring_error)
    }
}

impl CredentialVault for SystemCredentialVault {
    fn get(&self, credential_ref: &CredentialRef) -> Result<Option<SecretString>, VaultError> {
        let entry = Self::entry(credential_ref)?;
        let mut bytes = match entry.get_secret() {
            Ok(bytes) => bytes,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(error) => return Err(map_keyring_error(error)),
        };
        decode_secret(&mut bytes).map(Some)
    }

    fn put(&self, credential_ref: &CredentialRef, value: &SecretString) -> Result<(), VaultError> {
        Self::entry(credential_ref)?
            .set_secret(value.expose_secret().as_bytes())
            .map_err(map_keyring_error)
    }

    fn delete(&self, credential_ref: &CredentialRef) -> Result<(), VaultError> {
        match Self::entry(credential_ref)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring_error(error)),
        }
    }
}

fn decode_secret(bytes: &mut Vec<u8>) -> Result<SecretString, VaultError> {
    let secret = std::str::from_utf8(bytes)
        .map(|value| SecretString::from(value.to_owned()))
        .map_err(|_| VaultError::Platform);
    bytes.zeroize();
    secret
}

fn map_keyring_error(error: keyring::Error) -> VaultError {
    match error {
        keyring::Error::NoDefaultStore | keyring::Error::NotSupportedByStore(_) => {
            VaultError::Unavailable
        }
        keyring::Error::NoEntry => VaultError::NoEntry,
        keyring::Error::NoStorageAccess(_) => VaultError::Denied,
        keyring::Error::PlatformFailure(_) => VaultError::Platform,
        _ => VaultError::Platform,
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::{VaultError, decode_secret};

    #[test]
    fn decoding_zeroizes_source_on_success_and_invalid_utf8() {
        let mut valid = b"vault-unit-secret".to_vec();
        let decoded = decode_secret(&mut valid).unwrap();
        assert_eq!(decoded.expose_secret(), "vault-unit-secret");
        assert!(valid.is_empty() || valid.iter().all(|byte| *byte == 0));

        let mut invalid = vec![0xff, 0xfe];
        assert_eq!(
            decode_secret(&mut invalid).unwrap_err(),
            VaultError::Platform
        );
        assert!(invalid.is_empty() || invalid.iter().all(|byte| *byte == 0));
    }
}
