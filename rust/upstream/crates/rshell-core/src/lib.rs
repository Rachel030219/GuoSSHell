pub mod application;
pub mod connection;
pub mod error;
pub mod protocol;
pub mod render;
pub mod terminal;
pub mod workspace;

pub use application::{
    AppBootstrapState, AppDependencies, AppError, AppEventStream, AppViewModel, ApplicationHandle,
    ApplicationService, ConnectionRepository, CredentialOperationError, CredentialPort,
    ErrorPaneView, ImportCommitResult, ImportError, ImportPort, LatestViewStream, PaneLaunchTarget,
    RepositoryError, SessionBinding, SessionPort, UI_COMMAND_CAPACITY, UiCommandPort, UiPortError,
    VaultFailure,
};
pub use connection::{
    AuthenticationKind, CatalogMutation, CatalogOutcome, ConnectionCatalog, ConnectionGroup,
    ConnectionId, ConnectionProfile, CredentialRef, GroupId, HostKeyPolicy, PaneId, SessionId,
    TerminalProfileId, TransportKind,
};
pub use error::DomainError;
pub use protocol::{
    AppEvent, AppFailure, AppFailureCategory, AuthPrompt, DisplayRecoveryNotice, HostKeyDecision,
    HostKeyPrompt, ImportCandidateId, ImportCandidateView, ImportPreviewId, ImportPreviewView,
    ImportReportView, ImportSourceKind, ImportWarningView, InteractionId, InteractionRequest,
    InteractionResponse, KeyboardInteractivePrompt, RecoveryAction, SecretUpdate, SessionUiCommand,
    SessionUiEvent, UiCommand,
};
pub use render::{
    CellAttributes, CellPosition, Color, CursorShape, DisplayRecovery, ExitStatus, MouseButton,
    MouseEventKind, RenderCell, RenderCursor, RenderFrame, RenderRow, SearchMatch, SearchQuery,
    SelectionRange, SessionFailure, SessionState, TerminalDisplayModes, TerminalInput,
    TerminalMouseEvent, TerminalSize, Viewport,
};
pub use terminal::{
    AppSettings, ColorScheme, KeyBinding, KeyCode, KeyModifiers, ResolvedTerminalProfile,
    SettingsValidationCode, SettingsValidationError, TerminalKeyAction, TerminalOverrides,
    TerminalProfile, TerminalSendSequence, TerminalSettingsV1, TerminalSettingsVersion,
    parse_terminal_key_action, validate_app_settings, validate_terminal_overrides,
    validate_terminal_profile, validate_terminal_settings,
};
pub use workspace::{PaneTree, SplitAxis, TabId, TabState, WorkspaceError, WorkspaceState};
