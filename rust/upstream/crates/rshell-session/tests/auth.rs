use std::path::PathBuf;

use rshell_core::{
    AuthenticationKind, ConnectionProfile, CredentialRef, InteractionResponse, TransportKind,
};
use rshell_session::{
    AuthPlan, AuthPlanError, KeyboardInteractiveResponseError, keyboard_interactive_request,
    validate_keyboard_interactive_response,
};
use rshell_storage::{
    CredentialVault, MemoryCredentialVault, MemoryVaultFault, VaultError, VaultOperation,
};
use secrecy::{ExposeSecret, SecretString};

const PASSWORD: &str = "password-sentinel-must-not-leak";
const PASSPHRASE: &str = "passphrase-sentinel-must-not-leak";

#[test]
fn auth_plan_source_never_shares_or_duplicates_secret_storage() {
    let source = include_str!("../src/auth.rs");
    let shared_secret_type = ["A", "rc<", "Secret", "String>"].concat();
    let duplication_method = ["dupli", "cate"].concat();

    assert!(
        !source.contains(&shared_secret_type),
        "auth plans must own secret storage"
    );
    assert!(
        !source.contains(&duplication_method),
        "auth plans must not duplicate secret storage"
    );
}

fn profile(transport: TransportKind, authentication: AuthenticationKind) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new("auth test", "auth.test");
    profile.transport = transport;
    profile.authentication = authentication;
    profile.identity_file = Some(PathBuf::from("test identity"));
    profile
}

fn vault_with(reference: &CredentialRef, value: &str) -> MemoryCredentialVault {
    let vault = MemoryCredentialVault::new();
    vault
        .put(reference, &SecretString::from(value.to_owned()))
        .unwrap();
    vault
}

#[test]
fn supported_profiles_make_non_cloning_auth_plans_and_read_vault_once_when_needed() {
    let password_ref = CredentialRef::new("password-ref");
    let password_vault = vault_with(&password_ref, PASSWORD);
    let mut password_profile = profile(TransportKind::NativeSsh, AuthenticationKind::Password);
    password_profile.credential_ref = Some(password_ref);
    let password_plan = AuthPlan::from_profile(&password_profile, &password_vault).unwrap();
    assert_eq!(password_vault.call_counts().get, 1);
    let AuthPlan::Password { password, .. } = password_plan else {
        panic!("expected password plan");
    };
    assert_eq!(password.expose_secret(), PASSWORD);

    let passphrase_ref = CredentialRef::new("passphrase-ref");
    let public_key_vault = vault_with(&passphrase_ref, PASSPHRASE);
    let mut public_key_profile = profile(TransportKind::NativeSsh, AuthenticationKind::PublicKey);
    public_key_profile.credential_ref = Some(passphrase_ref);
    let public_key_plan = AuthPlan::from_profile(&public_key_profile, &public_key_vault).unwrap();
    assert_eq!(public_key_vault.call_counts().get, 1);
    let AuthPlan::PublicKey {
        identity_file,
        passphrase: Some(passphrase),
        ..
    } = public_key_plan
    else {
        panic!("expected public-key plan with passphrase");
    };
    assert_eq!(identity_file, PathBuf::from("test identity"));
    assert_eq!(passphrase.expose_secret(), PASSPHRASE);

    let public_key_without_secret_vault = MemoryCredentialVault::new();
    let public_key_without_secret =
        profile(TransportKind::NativeSsh, AuthenticationKind::PublicKey);
    assert!(matches!(
        AuthPlan::from_profile(&public_key_without_secret, &public_key_without_secret_vault),
        Ok(AuthPlan::PublicKey {
            passphrase: None,
            ..
        })
    ));
    assert_eq!(public_key_without_secret_vault.call_counts().get, 0);

    let keyboard_vault = MemoryCredentialVault::new();
    let keyboard_profile = profile(
        TransportKind::NativeSsh,
        AuthenticationKind::KeyboardInteractive,
    );
    assert!(matches!(
        AuthPlan::from_profile(&keyboard_profile, &keyboard_vault),
        Ok(AuthPlan::KeyboardInteractive { .. })
    ));
    assert_eq!(keyboard_vault.call_counts().get, 0);

    let agent_vault = MemoryCredentialVault::new();
    let agent_profile = profile(TransportKind::SystemOpenSsh, AuthenticationKind::Agent);
    assert!(matches!(
        AuthPlan::from_profile(&agent_profile, &agent_vault),
        Ok(AuthPlan::Agent { .. })
    ));
    assert_eq!(agent_vault.call_counts().get, 0);

    let native_agent_vault = MemoryCredentialVault::new();
    let native_agent = profile(TransportKind::NativeSsh, AuthenticationKind::Agent);
    assert!(matches!(
        AuthPlan::from_profile(&native_agent, &native_agent_vault),
        Ok(AuthPlan::Agent { .. })
    ));
    assert_eq!(native_agent_vault.call_counts().get, 0);
}

#[test]
fn invalid_combinations_missing_credentials_and_vault_faults_are_classified_without_secrets() {
    let vault = MemoryCredentialVault::new();
    let system_password = profile(TransportKind::SystemOpenSsh, AuthenticationKind::Password);
    assert!(matches!(
        AuthPlan::from_profile(&system_password, &vault),
        Err(AuthPlanError::UnsupportedCombination { .. })
    ));

    let no_ref = profile(TransportKind::NativeSsh, AuthenticationKind::Password);
    assert!(matches!(
        AuthPlan::from_profile(&no_ref, &vault),
        Err(AuthPlanError::MissingCredentialRef { .. })
    ));

    let missing_ref = CredentialRef::new("missing");
    let mut missing = profile(TransportKind::NativeSsh, AuthenticationKind::Password);
    missing.credential_ref = Some(missing_ref);
    assert!(matches!(
        AuthPlan::from_profile(&missing, &vault),
        Err(AuthPlanError::CredentialMissing { .. })
    ));

    let fault_ref = CredentialRef::new("fault");
    let mut faulting = profile(TransportKind::NativeSsh, AuthenticationKind::Password);
    faulting.credential_ref = Some(fault_ref);
    vault.inject_fault(MemoryVaultFault::before(
        VaultOperation::Get,
        2,
        VaultError::Denied,
    ));
    let first = AuthPlan::from_profile(&faulting, &vault);
    assert!(matches!(first, Err(AuthPlanError::CredentialFault { .. })));
    assert_eq!(vault.call_counts().get, 2);
}

#[test]
fn public_key_passphrase_reference_requires_a_vault_secret() {
    let vault = MemoryCredentialVault::new();
    let mut profile = profile(TransportKind::NativeSsh, AuthenticationKind::PublicKey);
    profile.credential_ref = Some(CredentialRef::new("missing-passphrase"));

    assert!(matches!(
        AuthPlan::from_profile(&profile, &vault),
        Err(AuthPlanError::CredentialMissing { .. })
    ));
    assert_eq!(vault.call_counts().get, 1);
}

#[test]
fn public_key_blank_passphrase_reference_is_not_treated_as_an_absent_passphrase() {
    let vault = MemoryCredentialVault::new();
    let mut profile = profile(TransportKind::NativeSsh, AuthenticationKind::PublicKey);
    profile.credential_ref = Some(CredentialRef::new("   "));

    assert!(matches!(
        AuthPlan::from_profile(&profile, &vault),
        Err(AuthPlanError::MissingCredentialRef { .. })
    ));
    assert_eq!(vault.call_counts().get, 0);
}

#[test]
fn auth_plan_and_errors_redact_passwords_and_passphrases_in_debug_and_display() {
    let password_ref = CredentialRef::new("password-ref");
    let password_vault = vault_with(&password_ref, PASSWORD);
    let mut password_profile = profile(TransportKind::NativeSsh, AuthenticationKind::Password);
    password_profile.credential_ref = Some(password_ref);
    let password_plan = AuthPlan::from_profile(&password_profile, &password_vault).unwrap();

    let passphrase_ref = CredentialRef::new("passphrase-ref");
    let passphrase_vault = vault_with(&passphrase_ref, PASSPHRASE);
    let mut key_profile = profile(TransportKind::NativeSsh, AuthenticationKind::PublicKey);
    key_profile.credential_ref = Some(passphrase_ref);
    let key_plan = AuthPlan::from_profile(&key_profile, &passphrase_vault).unwrap();

    let error = AuthPlan::from_profile(
        &profile(TransportKind::SystemOpenSsh, AuthenticationKind::Password),
        &MemoryCredentialVault::new(),
    )
    .unwrap_err();
    for output in [
        format!("{password_plan:?}"),
        format!("{key_plan:?}"),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(
            !output.contains(PASSWORD),
            "password leaked in formatted output"
        );
        assert!(
            !output.contains(PASSPHRASE),
            "passphrase leaked in formatted output"
        );
        assert!(output.contains("[REDACTED]"), "redaction marker missing");
    }
}

#[test]
fn keyboard_interactive_preserves_labels_echo_flags_and_accepts_only_exact_answers() {
    let request = keyboard_interactive_request(
        "Authentication",
        "Answer the server prompts",
        [
            ("Visible answer".to_owned(), true),
            ("OTP".to_owned(), false),
        ],
    );
    assert_eq!(request.name, "Authentication");
    assert_eq!(request.instruction, "Answer the server prompts");
    assert_eq!(request.prompts.len(), 2);
    assert_eq!(request.prompts[0].label, "Visible answer");
    assert!(request.prompts[0].echo);
    assert_eq!(request.prompts[1].label, "OTP");
    assert!(!request.prompts[1].echo);

    let answers = validate_keyboard_interactive_response(
        &request,
        InteractionResponse::Answers(vec![
            SecretString::from("visible".to_owned()),
            SecretString::from("otp-secret".to_owned()),
        ]),
    )
    .unwrap();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[1].expose_secret(), "otp-secret");

    assert!(matches!(
        validate_keyboard_interactive_response(&request, InteractionResponse::Cancel),
        Err(KeyboardInteractiveResponseError::Cancelled)
    ));
    assert!(matches!(
        validate_keyboard_interactive_response(
            &request,
            InteractionResponse::Answers(vec![SecretString::from("only-one".to_owned())])
        ),
        Err(KeyboardInteractiveResponseError::AnswerCount { .. })
    ));
    assert!(matches!(
        validate_keyboard_interactive_response(
            &request,
            InteractionResponse::HostKey(rshell_core::HostKeyDecision::Reject)
        ),
        Err(KeyboardInteractiveResponseError::UnexpectedResponse)
    ));
}
