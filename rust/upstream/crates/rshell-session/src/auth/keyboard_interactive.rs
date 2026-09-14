use std::fmt;

use rshell_core::{AuthPrompt, InteractionId, InteractionResponse, KeyboardInteractivePrompt};
use secrecy::SecretString;

/// Builds a UI-safe keyboard-interactive challenge while retaining prompt order and echo flags.
pub fn keyboard_interactive_request(
    name: impl Into<String>,
    instruction: impl Into<String>,
    prompts: impl IntoIterator<Item = (String, bool)>,
) -> KeyboardInteractivePrompt {
    KeyboardInteractivePrompt {
        id: InteractionId::new(),
        name: name.into(),
        instruction: instruction.into(),
        prompts: prompts
            .into_iter()
            .map(|(label, echo)| AuthPrompt {
                id: InteractionId::new(),
                label,
                echo,
            })
            .collect(),
    }
}

pub enum KeyboardInteractiveResponseError {
    Cancelled,
    UnexpectedResponse,
    AnswerCount { expected: usize, actual: usize },
}

impl fmt::Debug for KeyboardInteractiveResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let category = match self {
            Self::Cancelled => "Cancelled",
            Self::UnexpectedResponse => "UnexpectedResponse",
            Self::AnswerCount { .. } => "AnswerCount",
        };
        formatter
            .debug_struct("KeyboardInteractiveResponseError")
            .field("category", &category)
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for KeyboardInteractiveResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let category = match self {
            Self::Cancelled => "cancelled",
            Self::UnexpectedResponse => "unexpected response",
            Self::AnswerCount { .. } => "incorrect answer count",
        };
        write!(
            formatter,
            "keyboard-interactive response rejected ({category}; [REDACTED])"
        )
    }
}

impl std::error::Error for KeyboardInteractiveResponseError {}

pub fn validate_keyboard_interactive_response(
    request: &KeyboardInteractivePrompt,
    response: InteractionResponse,
) -> Result<Vec<SecretString>, KeyboardInteractiveResponseError> {
    match response {
        InteractionResponse::Answers(answers) if answers.len() == request.prompts.len() => {
            Ok(answers)
        }
        InteractionResponse::Answers(answers) => {
            Err(KeyboardInteractiveResponseError::AnswerCount {
                expected: request.prompts.len(),
                actual: answers.len(),
            })
        }
        InteractionResponse::Cancel => Err(KeyboardInteractiveResponseError::Cancelled),
        InteractionResponse::HostKey(_) | InteractionResponse::Secret(_) => {
            Err(KeyboardInteractiveResponseError::UnexpectedResponse)
        }
    }
}
