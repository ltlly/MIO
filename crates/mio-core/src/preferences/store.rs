use std::path::Path;

use anyhow::{Context, Result, ensure};

use super::{
    io::{
        read_preferences, validate_encoded_budget, validate_loaded_preferences,
        validate_startup_config, validate_tls_certificates, validated_recent_project,
        write_preferences_with,
    },
    *,
};

impl StartupPreferences {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for UserPreferences {
    fn default() -> Self {
        // Safe MIO initial behavior; not evidence of External factory defaults.
        Self {
            version: VERSION,
            revision: 0,
            running: RunningAttackClose::Ask,
            finished: FinishedAttackClose::Ask,
            display: UserDisplaySettings::default(),
            message_editor: UserMessageEditorSettings::default(),
            misc: UserMiscSettings::default(),
            startup: StartupPreferences::default(),
            recent_projects: Vec::new(),
            custom_ca_ders: Vec::new(),
        }
    }
}

impl PreferencesStore {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        let mut store = Self {
            path,
            state: UserPreferencesState {
                preferences: UserPreferences::default(),
                load_error: None,
            },
        };
        store.query();
        store
    }

    pub(crate) fn query(&mut self) -> UserPreferencesState {
        if let Some(path) = &self.path {
            self.state = match read_preferences(path) {
                Ok(preferences) => UserPreferencesState {
                    preferences,
                    load_error: None,
                },
                Err(error) => UserPreferencesState {
                    preferences: UserPreferences::default(),
                    load_error: Some(format!("用户偏好未载入，使用询问策略：{error:#}")),
                },
            };
        }
        self.state.clone()
    }

    pub(crate) fn set(
        &mut self,
        expected: u64,
        running: RunningAttackClose,
        finished: FinishedAttackClose,
    ) -> Result<UserPreferences> {
        self.apply(|current| next_preferences(current, expected, running, finished))
    }

    pub(crate) fn set_workbench_settings(
        &mut self,
        expected: u64,
        display: UserDisplaySettings,
        message_editor: UserMessageEditorSettings,
        misc: UserMiscSettings,
    ) -> Result<UserPreferences> {
        display.validate()?;
        message_editor.validate()?;
        misc.validate()?;
        self.apply(|current| {
            ensure!(
                current.revision == expected,
                "用户偏好 revision 已改变；未覆盖，请重新读取后重试"
            );
            let revision = expected
                .checked_add(1)
                .context("用户偏好 revision 已耗尽")?;
            Ok(UserPreferences {
                version: VERSION,
                revision,
                display,
                message_editor,
                misc,
                ..current.clone()
            })
        })
    }

    pub(crate) fn set_startup(
        &mut self,
        expected: u64,
        default_config: StartupDefaultConfig,
        disable_extensions: bool,
    ) -> Result<UserPreferences> {
        self.apply(|current| {
            next_startup_preferences(current, expected, default_config, disable_extensions)
        })
    }

    pub(crate) fn set_tls_certificates(
        &mut self,
        expected: u64,
        certificates: Vec<Vec<u8>>,
    ) -> Result<UserPreferences> {
        validate_tls_certificates(&certificates)?;
        self.apply(|current| {
            ensure!(
                current.revision == expected,
                "用户偏好 revision 已改变；未覆盖，请重新读取后重试"
            );
            let revision = expected
                .checked_add(1)
                .context("用户偏好 revision 已耗尽")?;
            Ok(UserPreferences {
                revision,
                custom_ca_ders: certificates,
                ..current.clone()
            })
        })
    }

    pub(crate) fn effective_upstream_tls(
        &self,
        project: &crate::UpstreamTlsSettings,
    ) -> Result<mio_proxy::UpstreamTls> {
        project.validate()?;
        if project.insecure_skip_verify || project.has_custom_root_override() {
            return Ok(project.to_proxy_tls());
        }
        ensure!(
            self.state.load_error.is_none(),
            "用户 TLS 设置未确认，未使用默认信任替代；请重新读取用户设置"
        );
        if self.state.preferences.custom_ca_ders.is_empty() {
            Ok(mio_proxy::UpstreamTls::System)
        } else {
            Ok(mio_proxy::UpstreamTls::CustomRoots {
                certificates_der: self.state.preferences.custom_ca_ders.clone(),
                include_system: true,
            })
        }
    }

    pub(crate) fn record_recent(
        &mut self,
        name: impl Into<String>,
        path: PathBuf,
        last_accessed_unix_ms: i64,
    ) -> Result<UserPreferences> {
        let name = name.into();
        self.apply(|current| next_recent_preferences(current, name, path, last_accessed_unix_ms))
    }

    pub(crate) fn remove_recent(
        &mut self,
        expected: u64,
        path: &Path,
    ) -> Result<UserPreferences> {
        let target = path.to_path_buf();
        self.apply(|current| next_removed_recent_preferences(current, expected, &target))
    }

    pub(crate) fn prune_missing_recent(&mut self, expected: u64) -> Result<UserPreferences> {
        self.apply(|current| next_pruned_recent_preferences(current, expected))
    }

    fn apply(
        &mut self,
        update: impl FnOnce(&UserPreferences) -> Result<UserPreferences>,
    ) -> Result<UserPreferences> {
        let preferences = if let Some(path) = &self.path {
            write_preferences_with(path, update)?
        } else {
            ensure!(
                self.state.load_error.is_none(),
                "用户偏好未载入；原文件未修改"
            );
            let preferences = update(&self.state.preferences)?;
            validate_loaded_preferences(&preferences)?;
            let encoded = serde_json::to_vec(&preferences)?;
            validate_encoded_budget(&preferences, encoded.len())?;
            preferences
        };
        self.state = UserPreferencesState {
            preferences: preferences.clone(),
            load_error: None,
        };
        Ok(preferences)
    }
}

fn next_preferences(
    current: &UserPreferences,
    expected: u64,
    running: RunningAttackClose,
    finished: FinishedAttackClose,
) -> Result<UserPreferences> {
    ensure!(
        current.revision == expected,
        "用户偏好 revision 已改变；未覆盖，请重新读取后重试"
    );
    let revision = expected
        .checked_add(1)
        .context("用户偏好 revision 已耗尽")?;
    Ok(UserPreferences {
        version: VERSION,
        revision,
        running,
        finished,
        ..current.clone()
    })
}

fn next_startup_preferences(
    current: &UserPreferences,
    expected: u64,
    default_config: StartupDefaultConfig,
    disable_extensions: bool,
) -> Result<UserPreferences> {
    validate_startup_config(&default_config)?;
    ensure!(
        current.revision == expected,
        "用户偏好 revision 已改变；未覆盖，请重新读取后重试"
    );
    let revision = expected
        .checked_add(1)
        .context("用户偏好 revision 已耗尽")?;
    Ok(UserPreferences {
        version: VERSION,
        revision,
        startup: StartupPreferences {
            default_config: Some(default_config),
            disable_extensions,
        },
        ..current.clone()
    })
}

fn next_recent_preferences(
    current: &UserPreferences,
    name: String,
    path: PathBuf,
    last_accessed_unix_ms: i64,
) -> Result<UserPreferences> {
    let entry = validated_recent_project(name, path, last_accessed_unix_ms)?;
    let revision = current
        .revision
        .checked_add(1)
        .context("用户偏好 revision 已耗尽")?;
    let mut recent_projects =
        Vec::with_capacity((current.recent_projects.len() + 1).min(MAX_RECENT_PROJECTS));
    recent_projects.push(entry);
    for existing in &current.recent_projects {
        if existing.path != recent_projects[0].path {
            recent_projects.push(existing.clone());
        }
    }
    recent_projects.sort_by_key(|entry| std::cmp::Reverse(entry.last_accessed_unix_ms));
    recent_projects.truncate(MAX_RECENT_PROJECTS);
    Ok(UserPreferences {
        version: VERSION,
        revision,
        recent_projects,
        ..current.clone()
    })
}

fn next_removed_recent_preferences(
    current: &UserPreferences,
    expected: u64,
    path: &Path,
) -> Result<UserPreferences> {
    ensure!(
        current.revision == expected,
        "用户偏好 revision 已改变；未覆盖，请重新读取后重试"
    );
    let revision = expected
        .checked_add(1)
        .context("用户偏好 revision 已耗尽")?;
    let recent_projects = current
        .recent_projects
        .iter()
        .filter(|entry| entry.path != path)
        .cloned()
        .collect();
    Ok(UserPreferences {
        version: VERSION,
        revision,
        recent_projects,
        ..current.clone()
    })
}

fn next_pruned_recent_preferences(
    current: &UserPreferences,
    expected: u64,
) -> Result<UserPreferences> {
    ensure!(
        current.revision == expected,
        "用户偏好 revision 已改变；未覆盖，请重新读取后重试"
    );
    let revision = expected
        .checked_add(1)
        .context("用户偏好 revision 已耗尽")?;
    let recent_projects = current
        .recent_projects
        .iter()
        .filter(|entry| {
            !matches!(
                std::fs::metadata(&entry.path),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound
            )
        })
        .cloned()
        .collect();
    Ok(UserPreferences {
        version: VERSION,
        revision,
        recent_projects,
        ..current.clone()
    })
}
