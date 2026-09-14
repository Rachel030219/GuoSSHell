mod catalog;
mod credentials;
mod handle;
mod imports;
mod model;
mod pane_launch;
mod ports;
mod runtime;
mod session_commands;
mod session_events;
mod settings;
mod streams;
mod workspace;

pub use handle::{ApplicationHandle, ApplicationService};
pub use model::{AppBootstrapState, AppViewModel, ErrorPaneView, PaneLaunchTarget};
pub use ports::{
    AppDependencies, AppError, ConnectionRepository, CredentialOperationError, CredentialPort,
    ImportCommitResult, ImportError, ImportPort, RepositoryError, SessionBinding, SessionPort,
    UI_COMMAND_CAPACITY, UiCommandPort, UiPortError, VaultFailure,
};
pub use streams::{AppEventStream, LatestViewStream};
