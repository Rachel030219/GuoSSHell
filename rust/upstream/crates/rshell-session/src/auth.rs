mod error;
mod keyboard_interactive;

use std::{fmt, path::Path};

use rshell_core::{AuthenticationKind, ConnectionProfile, TransportKind};
use rshell_storage::CredentialVault;
use secrecy::SecretString;

pub use error::AuthPlanError;
pub use keyboard_interactive::{
    KeyboardInteractiveResponseError, keyboard_interactive_request,
    validate_keyboard_interactive_response,
};

/// Authentication material prepared once for a transport. Secret-bearing variants intentionally
/// own their values so native transport can consume them without cloning.
pub enum AuthPlan {
    Password {
        host: String,
        password: SecretString,
    },
    PublicKey {
        host: String,
        identity_file: std::path::PathBuf,
        passphrase: Option<SecretString>,
    },
    Agent {
        host: String,
    },
    KeyboardInteractive {
        host: String,
    },
}

impl AuthPlan {
    /// Builds an authentication plan from application-provided material without reading a vault.
    /// The optional secret is moved directly into the selected plan.
    pub fn from_secret(
        profile: &ConnectionProfile,
        secret: Option<SecretString>,
    ) -> Result<Self, AuthPlanError> {
        if !supported_combination(profile.transport, profile.authentication) {
            return Err(AuthPlanError::UnsupportedCombination {
                host: profile.host.clone(),
                transport: profile.transport,
                authentication: profile.authentication,
            });
        }
        let host = profile.host.clone();
        match profile.authentication {
            AuthenticationKind::Password => secret
                .map(|password| Self::Password { host, password })
                .ok_or(AuthPlanError::CredentialMissing {
                    host: profile.host.clone(),
                    authentication: profile.authentication,
                }),
            AuthenticationKind::PublicKey => {
                let identity_file = profile
                    .identity_file
                    .clone()
                    .filter(|path| has_path_text(path))
                    .ok_or(AuthPlanError::MissingIdentityFile {
                        host: host.clone(),
                        authentication: profile.authentication,
                    })?;
                Ok(Self::PublicKey {
                    host,
                    identity_file,
                    passphrase: secret,
                })
            }
            AuthenticationKind::Agent => Ok(Self::Agent { host }),
            AuthenticationKind::KeyboardInteractive => Ok(Self::KeyboardInteractive { host }),
        }
    }

    pub fn from_profile(
        profile: &ConnectionProfile,
        vault: &dyn CredentialVault,
    ) -> Result<Self, AuthPlanError> {
        if !supported_combination(profile.transport, profile.authentication) {
            return Err(AuthPlanError::UnsupportedCombination {
                host: profile.host.clone(),
                transport: profile.transport,
                authentication: profile.authentication,
            });
        }

        let host = profile.host.clone();
        match profile.authentication {
            AuthenticationKind::Password => {
                let password = required_secret(profile, vault)?;
                Ok(Self::Password { host, password })
            }
            AuthenticationKind::PublicKey => {
                let identity_file = profile
                    .identity_file
                    .clone()
                    .filter(|path| has_path_text(path))
                    .ok_or(AuthPlanError::MissingIdentityFile {
                        host: host.clone(),
                        authentication: profile.authentication,
                    })?;
                let passphrase = optional_secret(profile, vault)?;
                Ok(Self::PublicKey {
                    host,
                    identity_file,
                    passphrase,
                })
            }
            AuthenticationKind::Agent => Ok(Self::Agent { host }),
            AuthenticationKind::KeyboardInteractive => Ok(Self::KeyboardInteractive { host }),
        }
    }

    pub fn kind(&self) -> AuthenticationKind {
        match self {
            Self::Password { .. } => AuthenticationKind::Password,
            Self::PublicKey { .. } => AuthenticationKind::PublicKey,
            Self::Agent { .. } => AuthenticationKind::Agent,
            Self::KeyboardInteractive { .. } => AuthenticationKind::KeyboardInteractive,
        }
    }

    pub fn host(&self) -> &str {
        match self {
            Self::Password { host, .. }
            | Self::PublicKey { host, .. }
            | Self::Agent { host }
            | Self::KeyboardInteractive { host } => host,
        }
    }
}

impl fmt::Debug for AuthPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthPlan")
            .field("kind", &self.kind())
            .field("host", &self.host())
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

fn supported_combination(transport: TransportKind, authentication: AuthenticationKind) -> bool {
    matches!(
        (transport, authentication),
        (
            TransportKind::SystemOpenSsh,
            AuthenticationKind::Agent | AuthenticationKind::PublicKey
        ) | (
            TransportKind::NativeSsh,
            AuthenticationKind::Password
                | AuthenticationKind::PublicKey
                | AuthenticationKind::Agent
                | AuthenticationKind::KeyboardInteractive
        )
    )
}

fn required_secret(
    profile: &ConnectionProfile,
    vault: &dyn CredentialVault,
) -> Result<SecretString, AuthPlanError> {
    let Some(reference) = profile
        .credential_ref
        .as_ref()
        .filter(|reference| !reference.0.trim().is_empty())
    else {
        return Err(AuthPlanError::MissingCredentialRef {
            host: profile.host.clone(),
            authentication: profile.authentication,
        });
    };
    vault
        .get(reference)
        .map_err(|vault| AuthPlanError::CredentialFault {
            host: profile.host.clone(),
            authentication: profile.authentication,
            vault,
        })?
        .ok_or_else(|| AuthPlanError::CredentialMissing {
            host: profile.host.clone(),
            authentication: profile.authentication,
        })
}

fn optional_secret(
    profile: &ConnectionProfile,
    vault: &dyn CredentialVault,
) -> Result<Option<SecretString>, AuthPlanError> {
    let Some(reference) = profile.credential_ref.as_ref() else {
        return Ok(None);
    };
    if reference.0.trim().is_empty() {
        return Err(AuthPlanError::MissingCredentialRef {
            host: profile.host.clone(),
            authentication: profile.authentication,
        });
    }
    vault
        .get(reference)
        .map_err(|vault| AuthPlanError::CredentialFault {
            host: profile.host.clone(),
            authentication: profile.authentication,
            vault,
        })
        .and_then(|secret| {
            secret.ok_or_else(|| AuthPlanError::CredentialMissing {
                host: profile.host.clone(),
                authentication: profile.authentication,
            })
        })
        .map(Some)
}

fn has_path_text(path: &Path) -> bool {
    path.to_str()
        .map_or(!path.as_os_str().is_empty(), |text| !text.trim().is_empty())
}
