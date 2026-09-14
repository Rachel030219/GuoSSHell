CREATE TABLE IF NOT EXISTS schema_migrations(
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE terminal_profiles(
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    settings_json TEXT NOT NULL
);

CREATE TABLE app_settings(
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    default_terminal_profile TEXT NOT NULL REFERENCES terminal_profiles(id) ON DELETE RESTRICT,
    color_scheme TEXT NOT NULL,
    key_bindings_json TEXT NOT NULL
);

CREATE TABLE connection_groups(
    id TEXT PRIMARY KEY NOT NULL,
    parent_id TEXT REFERENCES connection_groups(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    position INTEGER NOT NULL
);

CREATE TABLE connections(
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT REFERENCES connection_groups(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
    username TEXT NOT NULL,
    transport TEXT NOT NULL,
    authentication TEXT NOT NULL,
    credential_ref TEXT,
    identity_file TEXT,
    host_key_policy TEXT NOT NULL,
    remote_command TEXT,
    note TEXT NOT NULL,
    position INTEGER NOT NULL,
    terminal_profile_id TEXT REFERENCES terminal_profiles(id) ON DELETE RESTRICT,
    terminal_overrides_json TEXT NOT NULL
);

CREATE TABLE connection_tags(
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY(connection_id, tag)
);

CREATE TABLE credential_operations(
    operation_id TEXT PRIMARY KEY,
    credential_ref TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('put_new', 'delete_old')),
    state TEXT NOT NULL CHECK(state IN ('prepared', 'vault_applied')),
    created_at TEXT NOT NULL
);

CREATE INDEX idx_connection_groups_parent_position
    ON connection_groups(parent_id, position);
CREATE INDEX idx_connections_group_position ON connections(group_id, position);
CREATE INDEX idx_connections_search ON connections(name, host, username);
CREATE INDEX idx_connection_tags_tag ON connection_tags(tag, connection_id);

INSERT INTO terminal_profiles(id, name, settings_json) VALUES(
    '00000000-0000-0000-0000-000000000001',
    'Default',
    '{"version":1,"terminal_type":"xterm-256color","initial_cols":120,"initial_rows":36,"scrollback_lines":6000,"font_family":"CaskaydiaCove NF Mono","font_size":18.0,"color_scheme":"default","key_bindings":[],"left_alt_as_meta":true,"right_alt_as_meta":true,"enable_csi_u":false,"enable_kitty_keyboard":false,"mouse_reporting":true,"scroll_on_output":true,"scroll_on_keypress":false,"answerback":"rsHell"}'
);

INSERT INTO app_settings(singleton, default_terminal_profile, color_scheme, key_bindings_json)
VALUES(1, '00000000-0000-0000-0000-000000000001', 'default',
       '{"version":1,"key_bindings":[]}');
