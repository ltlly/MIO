#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};

use super::*;

/// Resolves the application preference location without accessing it.
pub fn default_user_preferences_path() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_USER_PREFERENCES_PATH") {
        let path = PathBuf::from(custom);
        validate_recorded_path(&path, "用户偏好")?;
        return Ok(path);
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").context("无法确定 macOS 用户目录")?;
        Ok(Path::new(&home).join("Library/Application Support/MIO/preferences.json"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            let p = PathBuf::from(xdg);
            if p.is_absolute() {
                return Ok(p.join("mio/preferences.json"));
            }
        }
        let home = std::env::var_os("HOME").context("无法确定用户目录")?;
        Ok(Path::new(&home).join(".config/mio/preferences.json"))
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(appdata).join("MIO").join("preferences.json"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .filter(|v| !v.is_empty())
        {
            return Ok(PathBuf::from(profile)
                .join("AppData")
                .join("Roaming")
                .join("MIO")
                .join("preferences.json"));
        }
        anyhow::bail!("无法确定用户目录")
    }
}

fn is_cross_platform_absolute(path: &Path) -> bool {
    path.is_absolute() || path.as_os_str().as_encoded_bytes().starts_with(b"/")
}

fn validate_recorded_path(path: &Path, label: &str) -> Result<()> {
    let raw = path.as_os_str().as_encoded_bytes();
    ensure!(
        !raw.is_empty() && raw.len() <= MAX_PATH_BYTES && !raw.contains(&0),
        "{label}路径不能为空、超过上限或包含空字节"
    );
    ensure!(
        is_cross_platform_absolute(path),
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
    let mut other_fields = preferences.clone();
    other_fields.custom_ca_ders.clear();
    ensure!(
        serde_json::to_vec(&other_fields)?.len() as u64 <= MAX_BYTES,
        "用户偏好非证书字段超过 64 KiB；原文件未修改"
    );
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

fn validate_parent(path: &Path) -> Result<PathBuf> {
    ensure!(
        is_cross_platform_absolute(path),
        "用户偏好路径必须是绝对路径"
    );
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir)),
        "用户偏好路径不能包含父目录跳转"
    );
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("用户偏好路径缺少父目录")?;
    for ancestor in parent.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "用户偏好目录不得经过符号链接或特殊文件"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("无法检查用户偏好目录"),
        }
    }
    Ok(parent.to_owned())
}

fn checked_file(file: &File) -> Result<fs::Metadata> {
    let metadata = file.metadata()?;
    #[cfg(unix)]
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o002 == 0
            && (metadata.mode() & 0o020 == 0 || metadata.gid() == unsafe { libc::getegid() }),
        "用户偏好必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    #[cfg(not(unix))]
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "用户偏好必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    Ok(metadata)
}

pub(super) fn read_preferences(path: &Path) -> Result<UserPreferences> {
    validate_parent(path)?;
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "无法安全打开用户偏好文件"
        );
    }
    let mut open_opts = OpenOptions::new();
    open_opts.read(true);
    #[cfg(unix)]
    open_opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = match open_opts.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserPreferences::default());
        }
        Err(error) => return Err(error).context("无法安全打开用户偏好文件"),
    };
    ensure!(
        checked_file(&file)?.len() <= MAX_FILE_BYTES,
        "用户偏好文件超过有界读取限额"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "用户偏好文件在读取时超过有界读取限额"
    );
    let preferences: UserPreferences = serde_json::from_slice(&bytes)
        .context("用户偏好文件损坏或含未知字段/选项；原文件未修改")?;
    validate_loaded_preferences(&preferences)?;
    validate_encoded_budget(&preferences, bytes.len())?;
    Ok(preferences)
}

pub(super) fn write_preferences_with(
    path: &Path,
    update: impl FnOnce(&UserPreferences) -> Result<UserPreferences>,
) -> Result<UserPreferences> {
    let parent = validate_parent(path)?;
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&parent)?;
    validate_parent(path)?;
    #[cfg(unix)]
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&parent)?;
    #[cfg(unix)]
    let directory_identity = directory.metadata()?;
    #[cfg(unix)]
    ensure!(
        directory_identity.uid() == unsafe { libc::geteuid() }
            && directory_identity.mode() & 0o002 == 0
            && (directory_identity.mode() & 0o020 == 0
                || directory_identity.gid() == unsafe { libc::getegid() }),
        "用户偏好目录必须由当前用户拥有且不可被其他用户写入"
    );
    let lock_path = parent.join("preferences.lock");
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(&lock_path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "用户偏好锁文件无效"
        );
    }
    let mut lock_opts = OpenOptions::new();
    lock_opts
        .read(true)
        .write(true)
        .create(true)
        .truncate(false);
    #[cfg(unix)]
    lock_opts
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let lock = lock_opts.open(&lock_path)?;
    let lock_identity = checked_file(&lock)?;
    lock.try_lock()
        .map_err(|error| anyhow::anyhow!("用户偏好正由其他实例保存或锁不可用：{error}"))?;
    let visible_lock = fs::symlink_metadata(&lock_path)?;
    #[cfg(unix)]
    ensure!(
        visible_lock.dev() == lock_identity.dev() && visible_lock.ino() == lock_identity.ino(),
        "用户偏好锁文件已替换；未保存"
    );
    #[cfg(not(unix))]
    ensure!(
        visible_lock.is_file() && visible_lock.len() == lock_identity.len(),
        "用户偏好锁文件已替换；未保存"
    );
    let current = read_preferences(path)?;
    let next = update(&current)?;
    validate_loaded_preferences(&next)?;
    let encoded = serde_json::to_vec(&next)?;
    validate_encoded_budget(&next, encoded.len())?;
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    temporary.write_all(&encoded)?;
    temporary.as_file().sync_all()?;
    validate_parent(path)?;
    #[cfg(unix)]
    {
        let visible_directory = fs::symlink_metadata(&parent)?;
        ensure!(
            visible_directory.dev() == directory_identity.dev()
                && visible_directory.ino() == directory_identity.ino(),
            "用户偏好目录已替换；未保存"
        );
    }
    // Re-read while holding the stable sidecar lock before publishing the replacement.
    ensure!(
        read_preferences(path)? == current,
        "用户偏好文件已在保存期间改变；未覆盖"
    );
    let visible_lock = fs::symlink_metadata(&lock_path)?;
    #[cfg(unix)]
    ensure!(
        visible_lock.dev() == lock_identity.dev() && visible_lock.ino() == lock_identity.ino(),
        "用户偏好锁文件已替换；未保存"
    );
    #[cfg(not(unix))]
    ensure!(
        visible_lock.is_file() && visible_lock.len() == lock_identity.len(),
        "用户偏好锁文件已替换；未保存"
    );
    temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    directory
        .sync_all()
        .context("用户偏好已发布但目录同步失败；请重新读取确认")?;
    Ok(next)
}
