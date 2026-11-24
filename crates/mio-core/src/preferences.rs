mod io;
mod store;
mod user_settings;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tls_tests;
#[cfg(test)]
mod user_settings_tests;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use io::default_user_preferences_path;
pub use user_settings::{
    UserDisplaySettings, UserMessageEditorSettings, UserMiscSettings, UserThemeMode,
};

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 64 * 1024;
const MAX_TLS_CERTIFICATES: usize = 32;
const MAX_TLS_CERTIFICATE_BYTES: usize = 64 * 1024;
const MAX_FILE_BYTES: u64 =
    MAX_BYTES + (MAX_TLS_CERTIFICATES * MAX_TLS_CERTIFICATE_BYTES * 4) as u64 + 128;
pub const MAX_RECENT_PROJECTS: usize = 32;
const MAX_PROJECT_NAME_BYTES: usize = 1024;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunningAttackClose {
    #[default]
    Ask,
    Continue,
    Delete,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishedAttackClose {
    #[default]
    Ask,
    SaveProject,
    KeepMemory,
    Delete,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupDefaultConfig {
    #[default]
    Defaults,
    Project,
    ConfigFile {
        path: PathBuf,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupPreferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_config: Option<StartupDefaultConfig>,
    #[serde(default)]
    pub disable_extensions: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentProject {
    pub name: String,
    pub path: PathBuf,
    pub last_accessed_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserPreferences {
    pub version: u32,
    pub revision: u64,
    pub running: RunningAttackClose,
    pub finished: FinishedAttackClose,
    #[serde(default, skip_serializing_if = "UserDisplaySettings::is_default")]
    pub display: UserDisplaySettings,
    #[serde(default, skip_serializing_if = "UserMessageEditorSettings::is_default")]
    pub message_editor: UserMessageEditorSettings,
    #[serde(default, skip_serializing_if = "UserMiscSettings::is_default")]
    pub misc: UserMiscSettings,
    #[serde(default, skip_serializing_if = "StartupPreferences::is_default")]
    pub startup: StartupPreferences,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_projects: Vec<RecentProject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_ca_ders: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPreferencesState {
    pub preferences: UserPreferences,
    pub load_error: Option<String>,
}

type PreferencesFileStamp = (
    u64,
    Option<std::time::SystemTime>,
    Option<(u64, u64)>,
);

pub(crate) struct PreferencesStore {
    path: Option<PathBuf>,
    state: UserPreferencesState,
    file_stamp: Option<PreferencesFileStamp>,
}
