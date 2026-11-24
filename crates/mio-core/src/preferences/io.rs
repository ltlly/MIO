use std::path::Path;

use anyhow::{Context, Result, ensure};

use super::*;
use crate::fs_atomic;

/// Resolves the application preference location without accessing it.
pub fn default_user_preferences_path() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_USER_PREFERENCES_PATH") {
        let path = PathBuf::from(custom);
        validate_recorded_path(&path, "用户偏好")?;
        return Ok(path);
    }
    Ok(fs_atomic::platform_app_dir(fs_atomic::AppDirKind::Config)?.join("preferences.json"))
}

fn validate_recorded_path(path: &Path, label: &str) -> Result<()> {
    let raw = path.as_os_str().as_encoded_bytes();
    ensure!(
        !raw.is_empty() && raw.len() <= MAX_PATH_BYTES && !raw.contains(&0),
        "{label}路径不能为空、超过上限或包含空字节"
    );
    ensure!(
        fs_atomic::is_cross_platform_absolute(path),
        "{label}路径必须是绝对路径"
    );
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir)),
        "{label}路径不能包含父目录跳转"
    );
    Ok(())
}

pub(super) fn validate_startup_config(default_config: &StartupDefaultConfig) -> Result<()> {
    if let StartupDefaultConfig::ConfigFile { path } = default_config {
        validate_recorded_path(path, "启动配置文件")?;
    }
    Ok(())
}

pub(super) fn validated_recent_project(
    name: String,
    path: PathBuf,
    last_accessed_unix_ms: i64,
) -> Result<RecentProject> {
    validate_recorded_path(&path, "最近项目")?;
    ensure!(last_accessed_unix_ms >= 0, "最近项目访问时间不能为负数");
    let trimmed = name.trim();
    let resolved_name = if trimmed.is_empty() {
        path.file_stem()
            .or_else(|| path.file_name())
            .and_then(|part| part.to_str())
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .context("最近项目名称不能为空")?
            .to_owned()
    } else {
        trimmed.to_owned()
    };
    ensure!(
        resolved_name.len() <= MAX_PROJECT_NAME_BYTES
            && !resolved_name.chars().any(char::is_control),
        "最近项目名称不能包含控制字符或超过长度上限"
    );
    Ok(RecentProject {
        name: resolved_name,
        path,
        last_accessed_unix_ms,
    })
}

pub(super) fn validate_loaded_preferences(preferences: &UserPreferences) -> Result<()> {
    ensure!(
        preferences.version == VERSION,
        "不支持的用户偏好版本；原文件未修改"
    );
    preferences.display.validate()?;
    preferences.message_editor.validate()?;
    preferences.misc.validate()?;
    validate_tls_certificates(&preferences.custom_ca_ders)?;
    if let Some(default_config) = &preferences.startup.default_config {
        validate_startup_config(default_config)?;
    }
    ensure!(
        preferences.recent_projects.len() <= MAX_RECENT_PROJECTS,
        "最近项目数量超过 32 条上限；原文件未修改"
    );
    for (index, item) in preferences.recent_projects.iter().enumerate() {
        validate_recorded_path(&item.path, "最近项目")?;
        ensure!(
            !item.name.trim().is_empty()
                && item.name.len() <= MAX_PROJECT_NAME_BYTES
                && !item.name.chars().any(char::is_control)
                && item.last_accessed_unix_ms >= 0,
            "最近项目记录无效；原文件未修改"
        );
        ensure!(
            !preferences.recent_projects[..index]
                .iter()
                .any(|prior| prior.path == item.path),
            "最近项目包含重复路径；原文件未修改"
        );
    }
    Ok(())
}

pub(super) fn validate_tls_certificates(certificates: &[Vec<u8>]) -> Result<()> {
    ensure!(
        certificates.len() <= MAX_TLS_CERTIFICATES
            && certificates
                .iter()
                .all(|certificate| !certificate.is_empty()
                    && certificate.len() <= MAX_TLS_CERTIFICATE_BYTES),
        "自定义 TLS 根证书最多 32 个，且须为非空、不超过 64 KiB 的 DER 编码"
    );
    for (index, certificate) in certificates.iter().enumerate() {
        ensure!(
            !certificates[..index].contains(certificate),
            "自定义 TLS 根证书重复；未保存"
        );
    }
    if !certificates.is_empty() {
        mio_proxy::UpstreamTls::CustomRoots {
            certificates_der: certificates.to_vec(),
            include_system: true,
        }
        .validate()?;
    }
    Ok(())
}

pub(super) fn validate_encoded_budget(
    preferences: &UserPreferences,
    encoded_bytes: usize,
) -> Result<()> {
    let certificate_bytes = if preferences.custom_ca_ders.is_empty() {
        0
    } else {
        serde_json::to_vec(&preferences.custom_ca_ders)?.len() as u64
            + b",\"custom_ca_ders\":".len() as u64
    };
    ensure!(
        encoded_bytes as u64 <= MAX_BYTES + certificate_bytes,
        "用户偏好文件除证书编码外超过 64 KiB；原文件未修改"
    );
    Ok(())
}

fn stamp_from_metadata(meta: &std::fs::Metadata) -> PreferencesFileStamp {
    let modified = meta.modified().ok();
    #[cfg(unix)]
    let dev_ino = {
        use std::os::unix::fs::MetadataExt as _;
        Some((meta.dev(), meta.ino()))
    };
    #[cfg(not(unix))]
    let dev_ino = None;
    (meta.len(), modified, dev_ino)
}

fn read_preferences_inner(
    path: &Path,
    validate_parent: bool,
) -> Result<(UserPreferences, Option<PreferencesFileStamp>)> {
    let Some((bytes, meta)) =
        fs_atomic::read_bounded_owned_file_with_meta(path, MAX_FILE_BYTES, validate_parent, "用户偏好")?
    else {
        return Ok((UserPreferences::default(), None));
    };
    let preferences: UserPreferences = serde_json::from_slice(&bytes)
        .context("用户偏好文件损坏或含未知字段/选项；原文件未修改")?;
    validate_loaded_preferences(&preferences)?;
    validate_encoded_budget(&preferences, bytes.len())?;
    Ok((preferences, Some(stamp_from_metadata(&meta))))
}

pub(super) fn read_preferences_with_stamp(
    path: &Path,
) -> Result<(UserPreferences, Option<PreferencesFileStamp>)> {
    read_preferences_inner(path, true)
}

#[cfg(test)]
pub(super) fn read_preferences(path: &Path) -> Result<UserPreferences> {
    read_preferences_with_stamp(path).map(|(preferences, _)| preferences)
}

pub(super) fn write_preferences_with(
    path: &Path,
    update: impl FnOnce(&UserPreferences) -> Result<UserPreferences>,
) -> Result<(UserPreferences, Option<PreferencesFileStamp>)> {
    let (parent, dir) = fs_atomic::ensure_owned_parent_dir(path, "用户偏好")?;
    let _lock = fs_atomic::acquire_sidecar_lock(&parent, "preferences.lock", "用户偏好")?;
    let (current, _) = read_preferences_inner(path, false)?;
    let next = update(&current)?;
    validate_loaded_preferences(&next)?;
    let encoded = serde_json::to_vec(&next)?;
    validate_encoded_budget(&next, encoded.len())?;
    fs_atomic::atomic_write_bytes(&parent, path, &encoded, &dir, "用户偏好")?;
    let stamp = std::fs::symlink_metadata(path)
        .ok()
        .map(|meta| stamp_from_metadata(&meta));
    Ok((next, stamp))
}
