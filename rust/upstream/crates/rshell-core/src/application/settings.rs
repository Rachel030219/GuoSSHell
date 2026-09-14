use crate::{
    AppEvent, AppFailure, AppFailureCategory, AppSettings, TerminalProfile, validate_app_settings,
    validate_terminal_profile,
};

use super::{catalog::repository_failure, runtime::CommandLoop};

impl CommandLoop {
    pub(super) async fn save_terminal_profile(&mut self, profile: TerminalProfile) {
        if validate_terminal_profile(&profile).is_err() {
            self.fail(settings_failure()).await;
            return;
        }
        if let Err(error) = self
            .dependencies
            .repository
            .save_terminal_profile(profile.clone())
            .await
        {
            self.fail(repository_failure(error)).await;
            return;
        }
        match self
            .view_model
            .terminal_profiles
            .iter_mut()
            .find(|item| item.id == profile.id)
        {
            Some(existing) => *existing = profile,
            None => self.view_model.terminal_profiles.push(profile),
        }
        self.publish_view();
        self.emit(AppEvent::TerminalProfilesChanged(
            self.view_model.terminal_profiles.clone(),
        ))
        .await;
    }

    pub(super) async fn save_settings(&mut self, settings: AppSettings) {
        if validate_app_settings(&settings, &self.view_model.terminal_profiles).is_err() {
            self.fail(settings_failure()).await;
            return;
        }
        if let Err(error) = self
            .dependencies
            .repository
            .save_settings(settings.clone())
            .await
        {
            self.fail(repository_failure(error)).await;
            return;
        }
        self.view_model.settings = settings.clone();
        self.publish_view();
        self.emit(AppEvent::SettingsChanged(settings)).await;
    }
}

fn settings_failure() -> AppFailure {
    AppFailure::fatal(
        AppFailureCategory::Validation,
        "terminal settings are invalid",
    )
}
