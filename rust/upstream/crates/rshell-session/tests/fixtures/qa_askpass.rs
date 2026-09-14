use std::{env, process::ExitCode};

const SECRET_ENV_NAME: &str = "RSHELL_QA_ASKPASS_SECRET_ENV";

fn main() -> ExitCode {
    let Some(secret_name) = env::var_os(SECRET_ENV_NAME) else {
        return ExitCode::FAILURE;
    };
    if secret_name.is_empty() {
        return ExitCode::FAILURE;
    }
    let Some(secret) = env::var_os(secret_name) else {
        return ExitCode::FAILURE;
    };
    println!("{}", secret.to_string_lossy());
    ExitCode::SUCCESS
}
