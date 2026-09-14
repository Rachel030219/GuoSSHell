use std::{env, fs::OpenOptions, io, path::PathBuf, process::Command};

const INTERACTIVE_SHELLS: usize = 2;

fn main() {
    if run().is_err() {
        std::process::exit(2);
    }
}

fn run() -> io::Result<()> {
    let ordinal = claim_ordinal()?;
    if ordinal <= INTERACTIVE_SHELLS {
        delegate_to_shell()
    } else {
        Ok(())
    }
}

fn claim_ordinal() -> io::Result<usize> {
    let prefix = env::var_os("RSHELL_P0_SHELL_COUNTER_PREFIX")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "counter prefix"))?;
    for ordinal in 1..=256 {
        let path = PathBuf::from(format!("{}.{}", prefix.to_string_lossy(), ordinal));
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(_) => return Ok(ordinal),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("counter exhausted"))
}

fn delegate_to_shell() -> io::Result<()> {
    let shell = env::var_os("RSHELL_PWSH_BIN")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "interactive shell"))?;
    let status = Command::new(shell).args(env::args_os().skip(1)).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("interactive shell failed"))
    }
}
