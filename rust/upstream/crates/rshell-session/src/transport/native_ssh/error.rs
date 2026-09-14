use std::fmt;

use rshell_core::SessionFailure;

use crate::TransportError;

pub(super) struct NativeClientError {
    failure: SessionFailure,
}

impl NativeClientError {
    pub(super) const fn new(failure: SessionFailure) -> Self {
        Self { failure }
    }

    pub(super) const fn failure(&self) -> SessionFailure {
        self.failure
    }
}

impl From<russh::Error> for NativeClientError {
    fn from(error: russh::Error) -> Self {
        let failure = match error {
            russh::Error::Elapsed(_) => SessionFailure::Timeout,
            russh::Error::UnknownKey => SessionFailure::HostKeyRejected,
            russh::Error::KeyChanged { .. } => SessionFailure::HostKeyChanged,
            russh::Error::NoAuthMethod
            | russh::Error::NotAuthenticated
            | russh::Error::UnsupportedAuthMethod => SessionFailure::Authentication,
            russh::Error::ChannelOpenFailure(_)
            | russh::Error::WrongChannel
            | russh::Error::RequestDenied => SessionFailure::SshChannel,
            russh::Error::IO(_)
            | russh::Error::ConnectionTimeout
            | russh::Error::KeepaliveTimeout
            | russh::Error::InactivityTimeout
            | russh::Error::Disconnect
            | russh::Error::HUP
            | russh::Error::SendError
            | russh::Error::RecvError => SessionFailure::Network,
            _ => SessionFailure::Network,
        };
        Self::new(failure)
    }
}

impl fmt::Debug for NativeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeClientError")
            .field("failure", &self.failure)
            .finish()
    }
}

impl fmt::Display for NativeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "native SSH operation failed ({:?})",
            self.failure
        )
    }
}

impl std::error::Error for NativeClientError {}

impl From<NativeClientError> for TransportError {
    fn from(error: NativeClientError) -> Self {
        Self::new(error.failure())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_errors_are_classified_without_retaining_diagnostics() {
        let reset = russh::Error::IO(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "sensitive endpoint detail",
        ));
        let error = NativeClientError::from(reset);

        assert_eq!(error.failure(), SessionFailure::Network);
        assert_eq!(
            NativeClientError::from(russh::Error::ConnectionTimeout).failure(),
            SessionFailure::Network
        );
        assert!(!format!("{error:?} {error}").contains("sensitive endpoint detail"));
    }
}
