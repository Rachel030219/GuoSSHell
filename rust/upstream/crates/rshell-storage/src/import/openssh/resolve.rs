use std::{collections::BTreeSet, path::PathBuf};

use rshell_core::{AuthenticationKind, ConnectionId, ConnectionProfile, TransportKind};

use super::{
    OpenSshCandidate, OpenSshPreview,
    lexer::wildcard_matches,
    parser::{Config, Directive},
};
use crate::{ImportError, ImportWarning};

pub(super) fn preview(config: Config) -> Result<OpenSshPreview, ImportError> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for block in &config.blocks {
        for pattern in &block.patterns {
            if seen.insert(pattern.clone()) {
                candidates.push(resolve_candidate(pattern, &config));
            }
        }
    }
    Ok(OpenSshPreview {
        candidates,
        warnings: config.warnings,
    })
}

fn resolve_candidate(alias: &str, config: &Config) -> OpenSshCandidate {
    let mut resolved = Resolved::default();
    for directive in &config.globals {
        resolved.apply(directive);
    }
    for block in &config.blocks {
        if patterns_match(&block.patterns, alias) {
            for directive in &block.directives {
                resolved.apply(directive);
            }
        }
    }
    let mut warnings = config.warnings.clone();
    let mut importable = !is_template(alias);
    if alias.starts_with('-') {
        importable = false;
        add_warning(
            &mut warnings,
            ImportWarning::InvalidHost { host: alias.into() },
        );
    }
    let host_name = resolved.host_name.unwrap_or_else(|| alias.into());
    if has_dynamic_token(&host_name) {
        importable = false;
        add_dynamic(&mut warnings, "HostName", &host_name);
    } else if host_name.starts_with('-') {
        importable = false;
        add_warning(
            &mut warnings,
            ImportWarning::InvalidHost {
                host: host_name.clone(),
            },
        );
    }
    let (port, valid_port) = resolve_port(resolved.port.as_deref(), &mut warnings);
    importable &= valid_port;

    let user = resolved.user.unwrap_or_default();
    let user = if has_dynamic_token(&user) {
        add_dynamic(&mut warnings, "User", &user);
        String::new()
    } else {
        user
    };
    let identity_file = resolve_identity(&resolved.identities, &mut warnings);
    if resolved.identities.len() > 1 {
        add_warning(&mut warnings, ImportWarning::MultipleIdentityFiles);
    }
    if resolved.proxy_command {
        importable = false;
        add_warning(
            &mut warnings,
            ImportWarning::UnsupportedDirective {
                directive: "ProxyCommand".into(),
            },
        );
    }
    if let Some(proxy_jump) = &resolved.proxy_jump
        && has_dynamic_token(proxy_jump)
    {
        add_dynamic(&mut warnings, "ProxyJump", proxy_jump);
    }

    let mut profile = ConnectionProfile::new(alias, &host_name);
    profile.id = ConnectionId::new();
    profile.username = user.clone();
    profile.port = port;
    profile.identity_file = identity_file.clone();
    profile.transport = TransportKind::SystemOpenSsh;
    profile.authentication = if identity_file.is_some() {
        AuthenticationKind::PublicKey
    } else {
        AuthenticationKind::Agent
    };
    if resolved.proxy_jump.is_some() {
        profile.host = alias.into();
        add_warning(&mut warnings, ImportWarning::DependsOnOpenSshConfig);
    }
    OpenSshCandidate {
        id: profile.id,
        host_pattern: alias.into(),
        host_name,
        user,
        port,
        identity_file,
        proxy_jump: resolved.proxy_jump,
        importable,
        profile,
        warnings,
    }
}

#[derive(Default)]
struct Resolved {
    host_name: Option<String>,
    user: Option<String>,
    port: Option<String>,
    identities: Vec<String>,
    proxy_jump: Option<String>,
    proxy_command: bool,
}

impl Resolved {
    fn apply(&mut self, directive: &Directive) {
        let value = || directive.values.first().cloned();
        match directive.keyword.as_str() {
            "hostname" if self.host_name.is_none() => self.host_name = value(),
            "user" if self.user.is_none() => self.user = value(),
            "port" if self.port.is_none() => self.port = value(),
            "identityfile" => {
                if let Some(value) = value() {
                    self.identities.push(value);
                }
            }
            "proxyjump" if self.proxy_jump.is_none() => self.proxy_jump = value(),
            "proxycommand" => self.proxy_command = true,
            _ => {}
        }
    }
}

fn patterns_match(patterns: &[String], alias: &str) -> bool {
    let mut positive = false;
    for pattern in patterns {
        let (negated, pattern) = pattern
            .strip_prefix('!')
            .map_or((false, pattern.as_str()), |pattern| (true, pattern));
        if wildcard_matches(pattern, alias) {
            if negated {
                return false;
            }
            positive = true;
        }
    }
    positive
}

fn is_template(alias: &str) -> bool {
    alias.contains(['*', '?', '!'])
}

fn resolve_port(value: Option<&str>, warnings: &mut Vec<ImportWarning>) -> (u16, bool) {
    let Some(value) = value else {
        return (22, true);
    };
    if has_dynamic_token(value) {
        add_dynamic(warnings, "Port", value);
        return (22, false);
    }
    match value.parse::<u32>() {
        Ok(port @ 1..=65535) => (port as u16, true),
        _ => {
            add_warning(
                warnings,
                ImportWarning::InvalidPort {
                    value: value.into(),
                },
            );
            (22, false)
        }
    }
}

fn resolve_identity(values: &[String], warnings: &mut Vec<ImportWarning>) -> Option<PathBuf> {
    let value = values.first()?;
    if value.starts_with('~') || has_dynamic_token(value) {
        add_dynamic(warnings, "IdentityFile", value);
        return None;
    }
    Some(PathBuf::from(value))
}

fn has_dynamic_token(value: &str) -> bool {
    ["%h", "%n", "%r", "%p", "%d", "%u", "%C"]
        .iter()
        .any(|token| value.contains(token))
}

fn add_dynamic(warnings: &mut Vec<ImportWarning>, directive: &str, value: &str) {
    add_warning(
        warnings,
        ImportWarning::DynamicValue {
            directive: directive.into(),
            value: value.into(),
        },
    );
}

fn add_warning(warnings: &mut Vec<ImportWarning>, warning: ImportWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
