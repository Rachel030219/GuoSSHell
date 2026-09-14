use std::{collections::BTreeSet, fmt, path::PathBuf};

use crate::{
    connection::{CatalogMutation, ConnectionCatalog, ConnectionId, PaneId, SessionId},
    terminal::{AppSettings, TerminalProfile},
    workspace::{SplitAxis, WorkspaceState},
};

use super::{
    AppFailure, ImportCandidateId, ImportPreviewId, ImportPreviewView, ImportReportView,
    ImportSourceKind, InteractionId, InteractionRequest, InteractionResponse, SecretUpdate,
    SessionUiCommand, SessionUiEvent,
};

// Keep payloads direct so the stable UI protocol matches its public shape.
#[allow(clippy::large_enum_variant)]
pub enum UiCommand {
    ApplyCatalog {
        mutation: CatalogMutation,
        secret: SecretUpdate,
    },
    SearchConnections(String),
    NewLocalTab,
    StartLocal {
        pane: PaneId,
    },
    Connect {
        pane: PaneId,
        connection: ConnectionId,
    },
    Split {
        pane: PaneId,
        axis: SplitAxis,
    },
    ClosePane(PaneId),
    CloseTab(uuid::Uuid),
    RetryPane(PaneId),
    Session {
        session: SessionId,
        command: SessionUiCommand,
    },
    SaveTerminalProfile(TerminalProfile),
    SaveSettings(AppSettings),
    PreviewImport {
        source: ImportSourceKind,
        path: PathBuf,
    },
    CommitImport {
        preview: ImportPreviewId,
        selected: BTreeSet<ImportCandidateId>,
    },
    CancelImport {
        preview: ImportPreviewId,
    },
    Respond {
        session: SessionId,
        interaction: InteractionId,
        response: InteractionResponse,
    },
    Shutdown,
}

impl UiCommand {
    pub fn secret_update(&self) -> Option<&SecretUpdate> {
        match self {
            Self::ApplyCatalog { secret, .. } => Some(secret),
            _ => None,
        }
    }
}

impl fmt::Debug for UiCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplyCatalog { mutation, secret } => formatter
                .debug_struct("ApplyCatalog")
                .field("mutation", mutation)
                .field("secret", secret)
                .finish(),
            Self::SearchConnections(query) => formatter
                .debug_tuple("SearchConnections")
                .field(query)
                .finish(),
            Self::NewLocalTab => formatter.write_str("NewLocalTab"),
            Self::StartLocal { pane } => formatter
                .debug_struct("StartLocal")
                .field("pane", pane)
                .finish(),
            Self::Connect { pane, connection } => formatter
                .debug_struct("Connect")
                .field("pane", pane)
                .field("connection", connection)
                .finish(),
            Self::Split { pane, axis } => formatter
                .debug_struct("Split")
                .field("pane", pane)
                .field("axis", axis)
                .finish(),
            Self::ClosePane(pane) => formatter.debug_tuple("ClosePane").field(pane).finish(),
            Self::CloseTab(tab) => formatter.debug_tuple("CloseTab").field(tab).finish(),
            Self::RetryPane(pane) => formatter.debug_tuple("RetryPane").field(pane).finish(),
            Self::Session { session, command } => formatter
                .debug_struct("Session")
                .field("session", session)
                .field("command", command)
                .finish(),
            Self::SaveTerminalProfile(profile) => formatter
                .debug_tuple("SaveTerminalProfile")
                .field(profile)
                .finish(),
            Self::SaveSettings(settings) => formatter
                .debug_tuple("SaveSettings")
                .field(settings)
                .finish(),
            Self::PreviewImport { source, path: _ } => formatter
                .debug_struct("PreviewImport")
                .field("source", source)
                .field("path", &"[REDACTED]")
                .finish(),
            Self::CommitImport { preview, selected } => formatter
                .debug_struct("CommitImport")
                .field("preview", preview)
                .field("selected", selected)
                .finish(),
            Self::CancelImport { preview } => formatter
                .debug_struct("CancelImport")
                .field("preview", preview)
                .finish(),
            Self::Respond {
                session,
                interaction,
                response,
            } => formatter
                .debug_struct("Respond")
                .field("session", session)
                .field("interaction", interaction)
                .field("response", response)
                .finish(),
            Self::Shutdown => formatter.write_str("Shutdown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    CatalogChanged(ConnectionCatalog),
    SearchResults(Vec<ConnectionId>),
    WorkspaceChanged(WorkspaceState),
    Session {
        session: SessionId,
        event: SessionUiEvent,
    },
    SettingsChanged(AppSettings),
    TerminalProfilesChanged(Vec<TerminalProfile>),
    ImportPreview(ImportPreviewView),
    ImportCompleted(ImportReportView),
    ImportCancelled(ImportPreviewId),
    InteractionRequired {
        session: SessionId,
        request: InteractionRequest,
    },
    InteractionResponded {
        session: SessionId,
        interaction: InteractionId,
    },
    OperationFailed(AppFailure),
    ShutdownComplete,
}
