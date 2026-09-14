use rshell_core::{ConnectionProfile, SessionFailure, TerminalSize, TransportKind};

use crate::{AuthPlan, TransportError, TransportRequest};

pub(super) fn validate_profile(
    profile: &ConnectionProfile,
    auth: &AuthPlan,
) -> Result<(), TransportError> {
    if profile.transport != TransportKind::NativeSsh
        || profile.port == 0
        || invalid_text(&profile.host)
        || invalid_text(&profile.username)
        || profile.authentication != auth.kind()
        || profile.host != auth.host()
        || profile
            .remote_command
            .as_deref()
            .is_some_and(|command| invalid_text(command) || command.trim().is_empty())
    {
        return Err(TransportError::new(SessionFailure::Validation));
    }
    Ok(())
}

pub(super) fn validate_request(request: &TransportRequest) -> Result<(), TransportError> {
    validate_size(request.initial_size())
}

pub(super) fn validate_size(size: TerminalSize) -> Result<(), TransportError> {
    if size.cols == 0 || size.rows == 0 {
        return Err(TransportError::new(SessionFailure::Validation));
    }
    Ok(())
}

fn invalid_text(value: &str) -> bool {
    value.is_empty() || value.contains(['\0', '\r', '\n'])
}
