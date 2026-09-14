use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard},
};

use rshell_core::CredentialRef;
use secrecy::{ExposeSecret, SecretString};

use crate::vault::{CredentialVault, VaultError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultOperation {
    Get,
    Put,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultMutation {
    Put,
    Delete,
}

impl VaultMutation {
    const fn operation(self) -> VaultOperation {
        match self {
            Self::Put => VaultOperation::Put,
            Self::Delete => VaultOperation::Delete,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryVaultCallCounts {
    pub get: usize,
    pub put: usize,
    pub delete: usize,
}

impl MemoryVaultCallCounts {
    fn increment(&mut self, operation: VaultOperation) -> usize {
        let count = match operation {
            VaultOperation::Get => &mut self.get,
            VaultOperation::Put => &mut self.put,
            VaultOperation::Delete => &mut self.delete,
        };
        *count += 1;
        *count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryVaultFault {
    operation: VaultOperation,
    call: usize,
    timing: FaultTiming,
    error: VaultError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaultTiming {
    Before,
    AfterMutationResultUnknown,
}

impl MemoryVaultFault {
    pub fn before(operation: VaultOperation, call: usize, error: VaultError) -> Self {
        assert!(call > 0, "vault fault calls are 1-based");
        Self {
            operation,
            call,
            timing: FaultTiming::Before,
            error,
        }
    }

    pub fn after_mutation_result_unknown(
        operation: VaultMutation,
        call: usize,
        error: VaultError,
    ) -> Self {
        assert!(call > 0, "vault fault calls are 1-based");
        Self {
            operation: operation.operation(),
            call,
            timing: FaultTiming::AfterMutationResultUnknown,
            error,
        }
    }
}

#[derive(Default)]
struct MemoryVaultState {
    entries: BTreeMap<String, SecretString>,
    calls: MemoryVaultCallCounts,
    faults: Vec<MemoryVaultFault>,
}

impl MemoryVaultState {
    fn take_fault(
        &mut self,
        timing: FaultTiming,
        operation: VaultOperation,
        call: usize,
    ) -> Option<VaultError> {
        let position = self.faults.iter().position(|fault| {
            fault.timing == timing && fault.operation == operation && fault.call == call
        })?;
        Some(self.faults.remove(position).error)
    }
}

#[derive(Default)]
pub struct MemoryCredentialVault {
    state: Mutex<MemoryVaultState>,
}

impl MemoryCredentialVault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, credential_ref: &CredentialRef) -> bool {
        self.state().entries.contains_key(&credential_ref.0)
    }

    pub fn is_empty(&self) -> bool {
        self.state().entries.is_empty()
    }

    pub fn inject_fault(&self, fault: MemoryVaultFault) {
        self.state().faults.push(fault);
    }

    pub fn call_counts(&self) -> MemoryVaultCallCounts {
        self.state().calls
    }

    fn state(&self) -> MutexGuard<'_, MemoryVaultState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl CredentialVault for MemoryCredentialVault {
    fn get(&self, credential_ref: &CredentialRef) -> Result<Option<SecretString>, VaultError> {
        let mut state = self.state();
        let call = state.calls.increment(VaultOperation::Get);
        if let Some(error) = state.take_fault(FaultTiming::Before, VaultOperation::Get, call) {
            return Err(error);
        }
        Ok(state
            .entries
            .get(&credential_ref.0)
            .map(|secret| SecretString::from(secret.expose_secret().to_owned())))
    }

    fn put(&self, credential_ref: &CredentialRef, value: &SecretString) -> Result<(), VaultError> {
        let mut state = self.state();
        let call = state.calls.increment(VaultOperation::Put);
        if let Some(error) = state.take_fault(FaultTiming::Before, VaultOperation::Put, call) {
            return Err(error);
        }
        state.entries.insert(
            credential_ref.0.clone(),
            SecretString::from(value.expose_secret().to_owned()),
        );
        match state.take_fault(
            FaultTiming::AfterMutationResultUnknown,
            VaultOperation::Put,
            call,
        ) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn delete(&self, credential_ref: &CredentialRef) -> Result<(), VaultError> {
        let mut state = self.state();
        let call = state.calls.increment(VaultOperation::Delete);
        if let Some(error) = state.take_fault(FaultTiming::Before, VaultOperation::Delete, call) {
            return Err(error);
        }
        state.entries.remove(&credential_ref.0);
        match state.take_fault(
            FaultTiming::AfterMutationResultUnknown,
            VaultOperation::Delete,
            call,
        ) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
