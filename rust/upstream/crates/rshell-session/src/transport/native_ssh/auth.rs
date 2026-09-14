use std::sync::Arc;

use rshell_core::{InteractionRequest, SessionFailure};
use russh::{
    client::{self, AuthResult, KeyboardInteractiveAuthResponse},
    keys::{PrivateKeyWithHashAlg, load_secret_key},
};
use secrecy::ExposeSecret;

use crate::{
    AuthPlan, InteractionBroker, TransportError, keyboard_interactive_request,
    validate_keyboard_interactive_response,
};

use super::{error::NativeClientError, handler::StrictClientHandler};

pub(super) async fn authenticate(
    handle: &mut client::Handle<StrictClientHandler>,
    username: &str,
    auth: AuthPlan,
    interactions: &InteractionBroker,
) -> Result<(), TransportError> {
    match auth {
        AuthPlan::Password { password, .. } => {
            let result = handle
                .authenticate_password(username, password.expose_secret().to_owned())
                .await
                .map_err(map_client)?;
            drop(password);
            require_success(result)?;
        }
        AuthPlan::PublicKey {
            identity_file,
            passphrase,
            ..
        } => {
            let key = load_secret_key(
                identity_file,
                passphrase.as_ref().map(|secret| secret.expose_secret()),
            )
            .map_err(|_| authentication_error())?;
            drop(passphrase);
            let hash = if key.algorithm().is_rsa() {
                handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(map_client)?
                    .flatten()
            } else {
                None
            };
            let result = handle
                .authenticate_publickey(username, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(map_client)?;
            require_success(result)?;
        }
        AuthPlan::KeyboardInteractive { .. } => {
            authenticate_keyboard_interactive(handle, username, interactions).await?;
        }
        AuthPlan::Agent { .. } => {
            authenticate_agent(handle, username).await?;
        }
    }
    scrub_russh_auth_method(handle, username).await;
    Ok(())
}

async fn authenticate_agent(
    handle: &mut client::Handle<StrictClientHandler>,
    username: &str,
) -> Result<(), TransportError> {
    use russh::keys::agent::client::AgentClient;

    #[cfg(unix)]
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|_| authentication_error())?
        .dynamic();
    #[cfg(windows)]
    let mut agent = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
        .await
        .map_err(|_| authentication_error())?
        .dynamic();
    #[cfg(not(any(unix, windows)))]
    return Err(authentication_error());

    let identities = agent
        .request_identities()
        .await
        .map_err(|_| authentication_error())?;
    for identity in identities {
        let public_key = identity.public_key().into_owned();
        let hash = if public_key.algorithm().is_rsa() {
            handle
                .best_supported_rsa_hash()
                .await
                .map_err(map_client)?
                .flatten()
        } else {
            None
        };
        let result = handle
            .authenticate_publickey_with(username, public_key, hash, &mut agent)
            .await
            .map_err(|_| authentication_error())?;
        if result.success() {
            return Ok(());
        }
    }
    Err(authentication_error())
}

async fn authenticate_keyboard_interactive(
    handle: &mut client::Handle<StrictClientHandler>,
    username: &str,
    interactions: &InteractionBroker,
) -> Result<(), TransportError> {
    let mut response = handle
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(map_client)?;
    loop {
        response = match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(()),
            KeyboardInteractiveAuthResponse::Failure { .. } => {
                return Err(authentication_error());
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let request = keyboard_interactive_request(
                    name,
                    instructions,
                    prompts
                        .into_iter()
                        .map(|prompt| (prompt.prompt, prompt.echo)),
                );
                let response = interactions
                    .request(InteractionRequest::KeyboardInteractive(request.clone()))
                    .await
                    .map_err(|_| authentication_error())?;
                let answers = validate_keyboard_interactive_response(&request, response)
                    .map_err(|_| authentication_error())?;
                let responses = answers
                    .iter()
                    .map(|answer| answer.expose_secret().to_owned())
                    .collect();
                handle
                    .authenticate_keyboard_interactive_respond(responses)
                    .await
                    .map_err(map_client)?
            }
        };
    }
}

async fn scrub_russh_auth_method(handle: &mut client::Handle<StrictClientHandler>, username: &str) {
    // russh retains its last Method after success. Queue a secret-free method while already
    // authenticated; the following channel-open is FIFO behind this message.
    let scrub = handle.authenticate_none(username.to_owned());
    tokio::pin!(scrub);
    tokio::select! {
        biased;
        _ = &mut scrub => {}
        _ = tokio::task::yield_now() => {}
    }
}

fn require_success(result: AuthResult) -> Result<(), TransportError> {
    if result.success() {
        Ok(())
    } else {
        Err(authentication_error())
    }
}

fn map_client(error: russh::Error) -> TransportError {
    TransportError::new(NativeClientError::from(error).failure())
}

fn authentication_error() -> TransportError {
    TransportError::new(SessionFailure::Authentication)
}
