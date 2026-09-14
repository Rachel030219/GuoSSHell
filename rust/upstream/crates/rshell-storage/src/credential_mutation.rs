use rshell_core::{CatalogMutation, ConnectionCatalog, CredentialRef, SecretUpdate};
use secrecy::SecretString;
use uuid::Uuid;

use crate::credential_types::CredentialOperationError;

pub(crate) const CREDENTIAL_REF_PREFIX: &str = "rshell://credential/";

pub(crate) enum PreparedMutation {
    NoPut(CatalogMutation),
    Put {
        mutation: CatalogMutation,
        reference: CredentialRef,
        secret: SecretString,
    },
}

pub(crate) fn prepare_mutation(
    catalog: &ConnectionCatalog,
    mutation: CatalogMutation,
    secret: SecretUpdate,
) -> Result<PreparedMutation, CredentialOperationError> {
    let prepared = match (mutation, secret) {
        (CatalogMutation::Create(mut profile), SecretUpdate::Set(secret)) => {
            let reference = new_reference();
            profile.credential_ref = Some(reference.clone());
            PreparedMutation::Put {
                mutation: CatalogMutation::Create(profile),
                reference,
                secret,
            }
        }
        (CatalogMutation::Update(mut profile), SecretUpdate::Set(secret)) => {
            let reference = new_reference();
            profile.credential_ref = Some(reference.clone());
            PreparedMutation::Put {
                mutation: CatalogMutation::Update(profile),
                reference,
                secret,
            }
        }
        (CatalogMutation::Update(mut profile), SecretUpdate::Unchanged) => {
            profile.credential_ref = catalog
                .connections
                .get(&profile.id)
                .ok_or(CredentialOperationError::Validation)?
                .credential_ref
                .clone();
            PreparedMutation::NoPut(CatalogMutation::Update(profile))
        }
        (CatalogMutation::Create(profile), SecretUpdate::Unchanged) => {
            if profile.credential_ref.as_ref().is_some_and(|reference| {
                !catalog
                    .connections
                    .values()
                    .any(|existing| existing.credential_ref.as_ref() == Some(reference))
            }) {
                return Err(CredentialOperationError::Validation);
            }
            PreparedMutation::NoPut(CatalogMutation::Create(profile))
        }
        (CatalogMutation::Update(mut profile), SecretUpdate::Clear) => {
            profile.credential_ref = None;
            PreparedMutation::NoPut(CatalogMutation::Update(profile))
        }
        (mutation, SecretUpdate::Unchanged) => PreparedMutation::NoPut(mutation),
        (_, SecretUpdate::Set(_) | SecretUpdate::Clear) => {
            return Err(CredentialOperationError::Validation);
        }
    };
    let mutation = match &prepared {
        PreparedMutation::NoPut(mutation) | PreparedMutation::Put { mutation, .. } => mutation,
    };
    let mut preview = catalog.clone();
    preview
        .apply(mutation.clone())
        .map_err(|_| CredentialOperationError::Validation)?;
    Ok(prepared)
}

pub(crate) fn new_reference() -> CredentialRef {
    #[cfg(feature = "test-support")]
    if let Some(reference) = injected_reference() {
        return reference;
    }
    CredentialRef::new(format!("{CREDENTIAL_REF_PREFIX}{}", Uuid::new_v4()))
}

#[cfg(feature = "test-support")]
static NEXT_REFERENCE: std::sync::OnceLock<std::sync::Mutex<Option<CredentialRef>>> =
    std::sync::OnceLock::new();

#[cfg(feature = "test-support")]
pub(crate) fn set_next_reference(reference: CredentialRef) {
    let mut next = NEXT_REFERENCE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        next.is_none(),
        "next credential reference is already injected"
    );
    *next = Some(reference);
}

#[cfg(feature = "test-support")]
fn injected_reference() -> Option<CredentialRef> {
    NEXT_REFERENCE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}
