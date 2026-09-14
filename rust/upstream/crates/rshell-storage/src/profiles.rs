use rshell_core::{AppSettings, TerminalProfile, TerminalSettingsV1};
use rusqlite::{Connection, params};

use crate::{StorageError, error, mapping, transaction};

pub(crate) fn load_profiles(connection: &Connection) -> Result<Vec<TerminalProfile>, StorageError> {
    let mut statement = connection
        .prepare("SELECT id, name, settings_json FROM terminal_profiles ORDER BY id")
        .map_err(error::sqlite)?;
    let mut rows = statement.query([]).map_err(error::sqlite)?;
    let mut profiles = Vec::new();
    while let Some(row) = rows.next().map_err(error::sqlite)? {
        let id: String = row.get(0).map_err(error::sqlite)?;
        let name: String = row.get(1).map_err(error::sqlite)?;
        let settings_json: String = row.get(2).map_err(error::sqlite)?;
        let settings: TerminalSettingsV1 =
            serde_json::from_str(&settings_json).map_err(|_| StorageError::Corrupt)?;
        profiles.push(TerminalProfile {
            id: mapping::profile_id(&id)?,
            name,
            settings,
        });
    }
    Ok(profiles)
}

pub(crate) fn save_profile(
    connection: &mut Connection,
    profile: TerminalProfile,
) -> Result<(), StorageError> {
    let settings =
        serde_json::to_string(&profile.settings).map_err(|_| StorageError::Serialization)?;
    transaction::immediate(connection, |transaction| {
        transaction
            .execute(
                "INSERT INTO terminal_profiles(id, name, settings_json) VALUES(?1, ?2, ?3) \
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, \
                 settings_json=excluded.settings_json",
                params![mapping::uuid_text(profile.id.0), profile.name, settings],
            )
            .map_err(error::sqlite)?;
        Ok(())
    })
}

pub(crate) fn load_settings(connection: &Connection) -> Result<AppSettings, StorageError> {
    let (default_profile, color_scheme, key_bindings) = connection
        .query_row(
            "SELECT default_terminal_profile, color_scheme, key_bindings_json \
             FROM app_settings WHERE singleton=1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(error::sqlite)?;
    mapping::app_settings(&default_profile, &color_scheme, &key_bindings)
}

pub(crate) fn save_settings(
    connection: &mut Connection,
    settings: AppSettings,
) -> Result<(), StorageError> {
    let key_bindings = mapping::key_bindings_json(&settings.key_bindings)?;
    transaction::immediate(connection, |transaction| {
        transaction
            .execute(
                "INSERT INTO app_settings(\
                 singleton, default_terminal_profile, color_scheme, key_bindings_json) \
                 VALUES(1, ?1, ?2, ?3) ON CONFLICT(singleton) DO UPDATE SET \
                 default_terminal_profile=excluded.default_terminal_profile, \
                 color_scheme=excluded.color_scheme, \
                 key_bindings_json=excluded.key_bindings_json",
                params![
                    mapping::uuid_text(settings.default_terminal_profile.0),
                    mapping::color_text(settings.color_scheme),
                    key_bindings,
                ],
            )
            .map_err(error::sqlite)?;
        Ok(())
    })
}
