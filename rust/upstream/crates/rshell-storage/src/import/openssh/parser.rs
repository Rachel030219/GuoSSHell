use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use super::lexer::{wildcard_matches, words};
use crate::{ImportError, ImportWarning};

const MAX_INCLUDE_DEPTH: usize = 8;

#[derive(Debug)]
pub(super) struct Config {
    pub(super) globals: Vec<Directive>,
    pub(super) blocks: Vec<HostBlock>,
    pub(super) warnings: Vec<ImportWarning>,
}

#[derive(Debug, Clone)]
pub(super) struct Directive {
    pub(super) keyword: String,
    pub(super) values: Vec<String>,
}

#[derive(Debug)]
pub(super) struct HostBlock {
    pub(super) patterns: Vec<String>,
    pub(super) directives: Vec<Directive>,
}

#[derive(Debug)]
struct FlatConfig {
    directives: Vec<Directive>,
    warnings: Vec<ImportWarning>,
}

pub(super) fn parse(path: &Path) -> Result<Config, ImportError> {
    let mut flat = FlatConfig {
        directives: Vec::new(),
        warnings: Vec::new(),
    };
    visit(path, 0, &mut Vec::new(), &mut BTreeSet::new(), &mut flat)?;
    Ok(section(flat))
}

fn visit(
    path: &Path,
    depth: usize,
    active: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<PathBuf>,
    flat: &mut FlatConfig,
) -> Result<(), ImportError> {
    let path = fs::canonicalize(path).map_err(|_| ImportError::Io)?;
    if active.contains(&path) {
        return Err(ImportError::IncludeCycle);
    }
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ImportError::IncludeDepth);
    }
    if !seen.insert(path.clone()) {
        return Ok(());
    }
    let source = fs::read_to_string(&path).map_err(|_| ImportError::Io)?;
    let parent = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    active.push(path);
    for line in source.lines() {
        let tokens = words(line);
        let Some((keyword, values)) = tokens.split_first() else {
            continue;
        };
        let keyword = keyword.to_ascii_lowercase();
        if keyword == "include" {
            for include in values {
                if has_dynamic_token(include) || include.starts_with('~') {
                    add_warning(
                        &mut flat.warnings,
                        ImportWarning::DynamicValue {
                            directive: "Include".into(),
                            value: include.clone(),
                        },
                    );
                    continue;
                }
                for child in included_paths(&parent, include) {
                    visit(&child, depth + 1, active, seen, flat)?;
                }
            }
        } else {
            flat.directives.push(Directive {
                keyword,
                values: values.to_vec(),
            });
        }
    }
    active.pop();
    Ok(())
}

fn included_paths(parent: &Path, include: &str) -> Vec<PathBuf> {
    let candidate = Path::new(include);
    let pattern = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        parent.join(candidate)
    };
    if !include.contains('*') && !include.contains('?') {
        return pattern.is_file().then_some(pattern).into_iter().collect();
    }
    let root = glob_root(&pattern);
    let mut matches = Vec::new();
    collect_matching_files(&root, &pattern, &mut matches);
    matches.sort_by_key(|path| normalized(path));
    matches
}

fn glob_root(pattern: &Path) -> PathBuf {
    let mut root = PathBuf::new();
    for component in pattern.components() {
        if matches!(component, Component::Normal(value) if value.to_string_lossy().contains(['*', '?']))
        {
            break;
        }
        root.push(component.as_os_str());
    }
    if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root
    }
}

fn collect_matching_files(root: &Path, pattern: &Path, matches: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_matching_files(&path, pattern, matches);
        } else if path.is_file() && wildcard_matches(&normalized(pattern), &normalized(&path)) {
            matches.push(path);
        }
    }
}

fn normalized(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized
        .strip_prefix("//?/")
        .unwrap_or(&normalized)
        .into()
}

fn section(flat: FlatConfig) -> Config {
    let mut globals = Vec::new();
    let mut blocks = Vec::new();
    let mut warnings = flat.warnings;
    let mut current = None;
    for directive in flat.directives {
        match directive.keyword.as_str() {
            "host" => {
                blocks.push(HostBlock {
                    patterns: directive.values,
                    directives: Vec::new(),
                });
                current = Some(blocks.len() - 1);
            }
            "match" => {
                add_warning(
                    &mut warnings,
                    ImportWarning::UnsupportedDirective {
                        directive: "Match".into(),
                    },
                );
                current = Some(usize::MAX);
            }
            _ => match current {
                None => globals.push(directive),
                Some(index) if index != usize::MAX => blocks[index].directives.push(directive),
                Some(_) => {}
            },
        }
    }
    Config {
        globals,
        blocks,
        warnings,
    }
}

fn has_dynamic_token(value: &str) -> bool {
    ["%h", "%n", "%r", "%p", "%d", "%u", "%C"]
        .iter()
        .any(|token| value.contains(token))
}

fn add_warning(warnings: &mut Vec<ImportWarning>, warning: ImportWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
