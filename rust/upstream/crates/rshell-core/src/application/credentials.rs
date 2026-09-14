use secrecy::SecretString;

use crate::{
    AppFailure, AppFailureCategory, AuthenticationKind, ConnectionId, ConnectionProfile,
    CredentialOperationError, CredentialPort, RecoveryAction, TransportKind,
};

pub(super) async fn launch_secret(
    credentials: &dyn CredentialPort,
    profile: &ConnectionProfile,
) -> Result<Option<SecretString>, AppFailure> {
    if profile.transport != TransportKind::NativeSsh
        || !matches!(
            profile.authentication,
            AuthenticationKind::Password | AuthenticationKind::PublicKey
        )
    {
        return Ok(None);
    }
    let Some(reference) = profile.credential_ref.as_ref() else {
        return Ok(None);
    };
    match credentials.get(reference).await {
        Ok(Some(secret)) => Ok(Some(secret)),
        Ok(None) => Err(lookup_failure(
            profile.id,
            AppFailureCategory::Vault,
            "credential is missing",
        )),
        Err(CredentialOperationError::Vault(_)) => Err(lookup_failure(
            profile.id,
            AppFailureCategory::Vault,
            "credential lookup failed",
        )),
        Err(
            CredentialOperationError::Repository(_)
            | CredentialOperationError::ReconciliationRequired,
        ) => Err(lookup_failure(
            profile.id,
            AppFailureCategory::Storage,
            "credential lookup failed",
        )),
    }
}

fn lookup_failure(
    connection: ConnectionId,
    category: AppFailureCategory,
    context: &'static str,
) -> AppFailure {
    AppFailure::retryable(
        category,
        context,
        RecoveryAction::EditConnection(connection),
    )
}
