use std::sync::Mutex;

use rshell_core::{ConnectionProfile, SessionFailure};

use crate::{
    AuthPlan, KnownHostsVerifier, NativeSshTransport, SessionTransport, TransportError,
    TransportFactory, TransportRequest,
};

/// Builds one native SSH transport from the authentication material supplied for a launch.
pub struct NativeFactory {
    profile: ConnectionProfile,
    auth: Mutex<Option<AuthPlan>>,
    verifier: KnownHostsVerifier,
}

impl NativeFactory {
    pub fn new(profile: ConnectionProfile, auth: AuthPlan, verifier: KnownHostsVerifier) -> Self {
        Self {
            profile,
            auth: Mutex::new(Some(auth)),
            verifier,
        }
    }

    #[cfg(test)]
    fn has_pending_auth_for_test(&self) -> bool {
        self.auth
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }
}

impl TransportFactory for NativeFactory {
    fn create(
        &self,
        _request: &TransportRequest,
    ) -> Result<Box<dyn SessionTransport>, TransportError> {
        let auth = self
            .auth
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .ok_or_else(|| TransportError::new(SessionFailure::Authentication))?;
        NativeSshTransport::new(self.profile.clone(), auth, self.verifier.clone())
            .map(|transport| Box::new(transport) as Box<dyn SessionTransport>)
    }
}

#[cfg(test)]
mod tests {
    use rshell_core::{AuthenticationKind, ConnectionProfile, TerminalSize, TransportKind};

    use super::*;

    #[test]
    fn native_factory_consumes_pending_auth_before_a_second_create() {
        let mut profile = ConnectionProfile::new("native", "native.test");
        profile.username = "native-user".into();
        profile.transport = TransportKind::NativeSsh;
        profile.authentication = AuthenticationKind::Agent;
        let auth = AuthPlan::from_secret(&profile, None).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let factory = NativeFactory::new(
            profile,
            auth,
            KnownHostsVerifier::new(directory.path().join("known_hosts")),
        );
        let request = TransportRequest::new(TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        });

        assert!(factory.has_pending_auth_for_test());
        let first = factory.create(&request).expect("initial native transport");
        assert!(!factory.has_pending_auth_for_test());
        let error = match factory.create(&request) {
            Ok(_) => panic!("native factory reused consumed authentication"),
            Err(error) => error,
        };
        assert_eq!(error.failure(), SessionFailure::Authentication);
        drop(first);
    }
}
