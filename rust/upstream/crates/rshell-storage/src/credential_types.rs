use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    AfterPrepare,
    AfterVaultPutBeforeState,
    AfterVaultApplied,
    AfterCatalogCommitBeforeCleanup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CredentialOperationError {
    Validation,
    Conflict,
    AlreadyImported,
    Vault,
    Storage,
    ReconciliationRequired,
    InjectedCrash(CrashPoint),
}

impl CredentialOperationError {
    const fn category(self) -> &'static str {
        match self {
            Self::Validation => "Validation",
            Self::Conflict => "Conflict",
            Self::AlreadyImported => "AlreadyImported",
            Self::Vault => "Vault",
            Self::Storage => "Storage",
            Self::ReconciliationRequired => "ReconciliationRequired",
            Self::InjectedCrash(_) => "InjectedCrash",
        }
    }
}

impl fmt::Debug for CredentialOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category())
    }
}

impl fmt::Display for CredentialOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category())
    }
}

impl std::error::Error for CredentialOperationError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub completed: usize,
    pub failed: usize,
    pub remaining: usize,
}

impl ReconcileReport {
    pub const fn is_converged(self) -> bool {
        self.failed == 0 && self.remaining == 0
    }
}
