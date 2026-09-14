use std::collections::BTreeMap;

use rshell_core::{
    ConnectionId, ImportCandidateId, ImportCandidateView, ImportPreviewId, ImportPreviewView,
    ImportReportView, ImportSourceKind, ImportWarningView,
};

use crate::{ImportPreview, ImportReport, ImportWarning, OpenSshPreview};

pub(super) fn legacy_view(
    id: ImportPreviewId,
    preview: &ImportPreview,
) -> (ImportPreviewView, BTreeMap<ImportCandidateId, ConnectionId>) {
    let pairs = preview
        .connections
        .iter()
        .map(|candidate| {
            let id = ImportCandidateId::new();
            (
                id,
                candidate.id,
                ImportCandidateView {
                    id,
                    name: candidate.profile.name.clone(),
                    host: candidate.profile.host.clone(),
                    port: candidate.profile.port,
                    username: candidate.profile.username.clone(),
                    source_label: "Legacy rsHell".into(),
                    has_secret: candidate.has_secret,
                    selectable: true,
                    authentication: candidate.profile.authentication,
                    credential_reference_present: candidate.profile.credential_ref.is_some(),
                    terminal_override_present: candidate.profile.terminal_overrides
                        != Default::default(),
                    importable: true,
                    wildcard: false,
                    warnings: Vec::new(),
                },
            )
        })
        .collect::<Vec<_>>();
    build_view(
        id,
        ImportSourceKind::LegacyRshellJson,
        preview.groups.clone(),
        &preview.warnings,
        pairs,
    )
}

pub(super) fn openssh_view(
    id: ImportPreviewId,
    preview: &OpenSshPreview,
) -> (ImportPreviewView, BTreeMap<ImportCandidateId, ConnectionId>) {
    let pairs = preview
        .candidates
        .iter()
        .map(|candidate| {
            let id = ImportCandidateId::new();
            (
                id,
                candidate.id,
                ImportCandidateView {
                    id,
                    name: candidate.profile.name.clone(),
                    host: candidate.host_name.clone(),
                    port: candidate.port,
                    username: candidate.user.clone(),
                    source_label: "OpenSSH config".into(),
                    has_secret: false,
                    selectable: candidate.importable,
                    authentication: candidate.profile.authentication,
                    credential_reference_present: candidate.profile.credential_ref.is_some(),
                    terminal_override_present: candidate.profile.terminal_overrides
                        != Default::default(),
                    importable: candidate.importable,
                    wildcard: is_wildcard_pattern(&candidate.host_pattern),
                    warnings: candidate.warnings.iter().map(warning_view).collect(),
                },
            )
        })
        .collect::<Vec<_>>();
    build_view(
        id,
        ImportSourceKind::OpenSshConfig,
        Vec::new(),
        &preview.warnings,
        pairs,
    )
}

fn is_wildcard_pattern(pattern: &str) -> bool {
    pattern
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'!'))
}

fn build_view(
    id: ImportPreviewId,
    source: ImportSourceKind,
    groups: Vec<rshell_core::ConnectionGroup>,
    warnings: &[ImportWarning],
    pairs: Vec<(ImportCandidateId, ConnectionId, ImportCandidateView)>,
) -> (ImportPreviewView, BTreeMap<ImportCandidateId, ConnectionId>) {
    let candidates = pairs.iter().map(|(_, _, view)| view.clone()).collect();
    let mapping = pairs
        .into_iter()
        .map(|(id, connection, _)| (id, connection))
        .collect();
    (
        ImportPreviewView {
            id,
            source,
            groups,
            candidates,
            warnings: warnings.iter().map(warning_view).collect(),
        },
        mapping,
    )
}

fn warning_view(warning: &ImportWarning) -> ImportWarningView {
    let (code, message) = match warning {
        ImportWarning::RecoveredFromBackup => ("recovered_backup", "Recovered from backup"),
        ImportWarning::HostKeyPolicyUpgraded => ("host_key_policy", "Host key policy was upgraded"),
        ImportWarning::KittyGraphicsDisabled => ("kitty_graphics", "Kitty graphics was disabled"),
        ImportWarning::DependsOnOpenSshConfig => {
            ("openssh_dependency", "Depends on OpenSSH configuration")
        }
        ImportWarning::MultipleIdentityFiles => (
            "multiple_identity_files",
            "Multiple identity files were reduced",
        ),
        ImportWarning::UnsupportedDirective { .. } => (
            "unsupported_directive",
            "An unsupported directive was ignored",
        ),
        ImportWarning::DynamicValue { .. } => {
            ("dynamic_value", "A dynamic value could not be imported")
        }
        ImportWarning::InvalidHost { .. } => ("invalid_host", "A host entry is invalid"),
        ImportWarning::InvalidPort { .. } => ("invalid_port", "A port entry is invalid"),
    };
    ImportWarningView {
        code: code.into(),
        message: message.into(),
    }
}

pub(super) fn report_view(report: ImportReport) -> ImportReportView {
    ImportReportView {
        imported_groups: report.imported_groups,
        imported_connections: report.imported_connections,
        skipped_candidates: report.skipped_connections,
    }
}
