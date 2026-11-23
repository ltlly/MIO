#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use mio_plugin_api::{Uuid, protocol::Permission};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::VerifiedPackage;

const REGISTRY_VERSION: u32 = 1;
const MAX_INSTALLED_PLUGINS: usize = 64;
const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
const MAX_INSTALL_FILES: usize = 1024;
const MAX_INSTALL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledPluginsFile {
    version: u32,
    #[serde(default)]
    plugins: Vec<InstalledPluginEntry>,
}

impl Default for InstalledPluginsFile {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            plugins: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPluginEntry {
    pub id: String,
    pub version: String,
    pub engine: String,
    pub package_dir: String,
    #[serde(default)]
    pub original_source: Option<PathBuf>,
    pub grants: Vec<Permission>,
    pub enabled: bool,
    #[serde(default)]
    pub auto_reload: bool,
}

#[derive(Clone, Debug)]
pub struct InstalledPackageStaging {
    pub final_dir: PathBuf,
    pub package_dir: String,
    backup_dir: Option<PathBuf>,
    newly_created: bool,
}

impl InstalledPackageStaging {
    pub fn commit(self) {
        if let Some(backup) = self.backup_dir {
            let _ = fs::remove_dir_all(backup);
        }
    }

    pub fn rollback(self) {
        if let Some(backup) = self.backup_dir {
            if fs::symlink_metadata(&self.final_dir).is_ok() {
                let _ = fs::remove_dir_all(&self.final_dir);
            }
            let _ = fs::rename(backup, &self.final_dir);
        } else if self.newly_created && fs::symlink_metadata(&self.final_dir).is_ok() {
            let _ = fs::remove_dir_all(&self.final_dir);
        }
    }
}

/// Resolves the user-level persistent plugins root directory without modifying the filesystem.
pub fn default_plugins_dir() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_PLUGINS_DIR").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(custom);
        validate_root_path(&path)?;
        return Ok(path);
    }
    let user_prefs = crate::preferences::default_user_preferences_path()?;
    let parent = user_prefs.parent().context("用户偏好路径缺少父目录")?;
    Ok(parent.join("plugins"))
}

#[derive(Clone, Debug, Default)]
pub struct PluginRegistryStore {
    root: Option<PathBuf>,
}

impl PluginRegistryStore {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self { root }
    }

    pub fn from_env_or_user_preferences(user_preferences_path: Option<&Path>) -> Self {
        if let Some(custom) = std::env::var_os("MIO_PLUGINS_DIR").filter(|v| !v.is_empty()) {
            return Self::new(Some(PathBuf::from(custom)));
        }
        Self::new(user_preferences_path.and_then(|p| p.parent().map(|dir| dir.join("plugins"))))
    }

    pub fn is_persistent(&self) -> bool {
        self.root.is_some()
    }

    pub fn root_dir(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn list(&self) -> Result<Vec<InstalledPluginEntry>> {
        let Some(root) = &self.root else {
            return Ok(Vec::new());
        };
        Ok(read_registry_file(root)?.plugins)
    }

    pub fn get(&self, plugin_id: &str) -> Result<Option<InstalledPluginEntry>> {
        validate_plugin_id(plugin_id)?;
        let Some(root) = &self.root else {
            return Ok(None);
        };
        Ok(read_registry_file(root)?
            .plugins
            .into_iter()
            .find(|entry| entry.id == plugin_id))
    }

    pub fn package_path(&self, entry: &InstalledPluginEntry) -> Result<PathBuf> {
        let root = self.root.as_ref().context("持久化插件目录未配置")?;
        validate_root_path(root)?;
        validate_segment(&entry.package_dir, "插件包目录名")?;
        Ok(root.join("packages").join(&entry.package_dir))
    }

    pub fn stage_install_package(
        &self,
        source: &VerifiedPackage,
    ) -> Result<Option<(VerifiedPackage, InstalledPackageStaging)>> {
        let Some(root) = &self.root else {
            return Ok(None);
        };
        validate_root_path(root)?;
        source.unchanged()?;
        validate_plugin_id(&source.manifest.id)?;
        let package_dir = source.manifest.id.clone();
        validate_segment(&package_dir, "插件包目录名")?;

        ensure_private_dir(root)?;
        let _registry_lock = acquire_registry_lock(root)?;
        let packages_dir = root.join("packages");
        ensure_private_dir(&packages_dir)?;

        let final_dir = packages_dir.join(&package_dir);
        if let Ok(final_canon) = fs::canonicalize(&final_dir)
            && let Ok(source_canon) = fs::canonicalize(source.work_dir())
            && final_canon == source_canon
        {
            return Ok(Some((
                source.clone(),
                InstalledPackageStaging {
                    final_dir,
                    package_dir,
                    backup_dir: None,
                    newly_created: false,
                },
            )));
        }

        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&packages_dir)
            .context("无法创建持久化插件暂存目录")?;
        #[cfg(unix)]
        fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))?;

        let mut total_files = 0usize;
        let mut total_bytes = 0u64;
        copy_dir_no_symlinks(
            source.work_dir(),
            staging.path(),
            &mut total_files,
            &mut total_bytes,
        )?;

        let staged_pkg = VerifiedPackage::read(staging.path())
            .context("复制后的持久化插件包校验失败")?;
        ensure!(
            staged_pkg.manifest_bytes == source.manifest_bytes,
            "持久化插件清单校验不一致"
        );
        ensure!(
            sha256_file(&staged_pkg.executable)? == sha256_file(&source.executable)?,
            "持久化插件入口校验和不一致"
        );

        let backup_dir = if let Ok(meta) = fs::symlink_metadata(&final_dir) {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "持久化插件目标路径存在异常文件或符号链接"
            );
            let backup = packages_dir.join(format!(".backup-{}", Uuid::new_v4()));
            fs::rename(&final_dir, &backup).context("无法备份旧版持久化插件目录")?;
            Some(backup)
        } else {
            None
        };

        let staged_path = staging.keep();
        if let Err(err) = fs::rename(&staged_path, &final_dir) {
            let _ = fs::remove_dir_all(&staged_path);
            if let Some(backup) = &backup_dir {
                let _ = fs::rename(backup, &final_dir);
            }
            return Err(err).context("无法激活持久化插件目录");
        }

        let installed_pkg = match VerifiedPackage::read(&final_dir) {
            Ok(pkg) => pkg,
            Err(err) => {
                if let Some(backup) = &backup_dir {
                    let _ = fs::remove_dir_all(&final_dir);
                    let _ = fs::rename(backup, &final_dir);
                } else {
                    let _ = fs::remove_dir_all(&final_dir);
                }
                return Err(err).context("已激活的持久化插件包无法重新校验");
            }
        };

        Ok(Some((
            installed_pkg,
            InstalledPackageStaging {
                final_dir,
                package_dir,
                newly_created: backup_dir.is_none(),
                backup_dir,
            },
        )))
    }

    pub fn upsert(&self, entry: InstalledPluginEntry) -> Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        validate_entry(&entry)?;
        write_registry_file_with(root, |current| {
            let mut next = current.clone();
            if let Some(existing) = next.plugins.iter_mut().find(|item| item.id == entry.id) {
                *existing = entry;
            } else {
                ensure!(
                    next.plugins.len() < MAX_INSTALLED_PLUGINS,
                    "持久化插件数量已达 64 个上限"
                );
                next.plugins.push(entry);
            }
            Ok(next)
        })
    }

    pub fn set_enabled(&self, plugin_id: &str, enabled: bool) -> Result<bool> {
        validate_plugin_id(plugin_id)?;
        let Some(root) = &self.root else {
            return Ok(false);
        };
        let mut updated = false;
        write_registry_file_with(root, |current| {
            let mut next = current.clone();
            if let Some(entry) = next.plugins.iter_mut().find(|item| item.id == plugin_id) {
                entry.enabled = enabled;
                updated = true;
            }
            Ok(next)
        })?;
        Ok(updated)
    }

    pub fn set_auto_reload(&self, plugin_id: &str, enabled: bool) -> Result<bool> {
        validate_plugin_id(plugin_id)?;
        let Some(root) = &self.root else {
            return Ok(false);
        };
        let mut updated = false;
        write_registry_file_with(root, |current| {
            let mut next = current.clone();
            if let Some(entry) = next.plugins.iter_mut().find(|item| item.id == plugin_id) {
                entry.auto_reload = enabled;
                updated = true;
            }
            Ok(next)
        })?;
        Ok(updated)
    }

    pub fn update_loaded_metadata(
        &self,
        plugin_id: &str,
        version: &str,
        grants: &[Permission],
    ) -> Result<bool> {
        validate_plugin_id(plugin_id)?;
        ensure!(
            !version.is_empty() && version.len() <= 64,
            "持久化插件版本无效"
        );
        let Some(root) = &self.root else {
            return Ok(false);
        };
        let mut updated = false;
        write_registry_file_with(root, |current| {
            let mut next = current.clone();
            if let Some(entry) = next.plugins.iter_mut().find(|item| item.id == plugin_id) {
                entry.version = version.to_owned();
                entry.grants = grants.to_vec();
                updated = true;
            }
            Ok(next)
        })?;
        Ok(updated)
    }

    pub fn reorder_by_ids(&self, ordered_live_ids: &[String]) -> Result<()> {
        for id in ordered_live_ids {
            validate_plugin_id(id)?;
        }
        let Some(root) = &self.root else {
            return Ok(());
        };
        write_registry_file_with(root, |current| {
            let mut next = current.clone();
            let mut by_id = HashMap::new();
            for entry in &next.plugins {
                by_id.insert(entry.id.clone(), entry.clone());
            }
            let live_set: BTreeSet<&str> = ordered_live_ids.iter().map(String::as_str).collect();
            let mut ordered_live: Vec<InstalledPluginEntry> = Vec::new();
            for id in ordered_live_ids {
                if let Some(entry) = by_id.get(id)
                    && !ordered_live.iter().any(|e| e.id == *id)
                {
                    ordered_live.push(entry.clone());
                }
            }
            let mut live_iter = ordered_live.into_iter();
            let mut rebuilt = Vec::with_capacity(next.plugins.len());
            for entry in &next.plugins {
                if live_set.contains(entry.id.as_str()) {
                    if let Some(replacement) = live_iter.next() {
                        rebuilt.push(replacement);
                    }
                } else {
                    rebuilt.push(entry.clone());
                }
            }
            for remaining in live_iter {
                rebuilt.push(remaining);
            }
            next.plugins = rebuilt;
            Ok(next)
        })
    }

    pub fn remove(&self, plugin_id: &str) -> Result<bool> {
        validate_plugin_id(plugin_id)?;
        let Some(root) = &self.root else {
            return Ok(false);
        };
        validate_root_path(root)?;
        ensure_private_dir(root)?;
        let registry_lock = acquire_registry_lock(root)?;
        let mut removed_entry: Option<InstalledPluginEntry> = None;
        write_registry_file_locked(root, &registry_lock, |current| {
            let mut next = current.clone();
            if let Some(pos) = next.plugins.iter().position(|item| item.id == plugin_id) {
                removed_entry = Some(next.plugins.remove(pos));
            }
            Ok(next)
        })?;
        if let Some(entry) = removed_entry {
            let target = self.package_path(&entry)?;
            match fs::symlink_metadata(&target) {
                Ok(meta) => {
                    ensure!(
                        meta.is_dir() && !meta.file_type().is_symlink(),
                        "持久化插件包目录异常或为符号链接，已拒绝删除"
                    );
                    fs::remove_dir_all(&target).context("删除持久化插件包目录失败")?;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err).context("检查待删除持久化插件目录失败"),
            }
            return Ok(true);
        }
        Ok(false)
    }
}

fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    ensure!(
        !plugin_id.is_empty()
            && plugin_id.len() <= 128
            && plugin_id != "."
            && plugin_id != ".."
            && plugin_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')),
        "插件 ID 无效"
    );
    Ok(())
}

fn validate_segment(segment: &str, label: &str) -> Result<()> {
    validate_plugin_id(segment).map_err(|_| anyhow::anyhow!("{label}无效"))?;
    let mut comps = Path::new(segment).components();
    ensure!(
        matches!(comps.next(), Some(Component::Normal(_))) && comps.next().is_none(),
        "{label}必须是单一安全目录名"
    );
    Ok(())
}

fn validate_root_path(root: &Path) -> Result<()> {
    let raw = root.as_os_str().as_encoded_bytes();
    ensure!(
        !raw.is_empty() && raw.len() <= MAX_PATH_BYTES && !raw.contains(&0),
        "持久化插件目录路径无效"
    );
    ensure!(
        root.is_absolute() || raw.starts_with(b"/"),
        "持久化插件目录必须是绝对路径"
    );
    ensure!(
        !root
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "持久化插件目录路径不能包含父目录跳转"
    );
    if let Ok(meta) = fs::symlink_metadata(root) {
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "持久化插件根目录不能是符号链接或普通文件"
        );
    }
    Ok(())
}

fn ensure_private_dir(dir: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "持久化插件目录不能是符号链接"
    );
    Ok(())
}

fn checked_regular_file(file: &File) -> Result<fs::Metadata> {
    let meta = file.metadata()?;
    #[cfg(unix)]
    ensure!(
        meta.is_file()
            && meta.nlink() == 1
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o002 == 0
            && (meta.mode() & 0o020 == 0 || meta.gid() == unsafe { libc::getegid() }),
        "持久化插件索引必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    #[cfg(not(unix))]
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "持久化插件索引必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    Ok(meta)
}

fn validate_entry(entry: &InstalledPluginEntry) -> Result<()> {
    validate_plugin_id(&entry.id)?;
    ensure!(
        !entry.version.is_empty() && entry.version.len() <= 64,
        "持久化插件版本无效"
    );
    ensure!(
        entry.engine == "native" || entry.engine == "wasm",
        "持久化插件引擎不受支持"
    );
    validate_segment(&entry.package_dir, "持久化插件包目录")?;
    let mut unique_grants = BTreeSet::new();
    for grant in &entry.grants {
        ensure!(unique_grants.insert(*grant), "持久化插件授权列表重复");
    }
    Ok(())
}

fn validate_registry_file(data: &InstalledPluginsFile) -> Result<()> {
    ensure!(
        data.version == REGISTRY_VERSION,
        "不支持的持久化插件清单版本；原文件未修改"
    );
    ensure!(
        data.plugins.len() <= MAX_INSTALLED_PLUGINS,
        "持久化插件数量超过 64 个限额；原文件未修改"
    );
    let mut seen_ids = BTreeSet::new();
    for entry in &data.plugins {
        validate_entry(entry)?;
        ensure!(
            seen_ids.insert(entry.id.as_str()),
            "持久化插件清单包含重复插件 ID；原文件未修改"
        );
    }
    Ok(())
}

fn read_registry_file(root: &Path) -> Result<InstalledPluginsFile> {
    validate_root_path(root)?;
    let path = root.join("installed.json");
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(&path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "无法安全打开持久化插件索引"
        );
    }
    let mut open_opts = OpenOptions::new();
    open_opts.read(true);
    #[cfg(unix)]
    open_opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = match open_opts.open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstalledPluginsFile::default());
        }
        Err(err) => return Err(err).context("无法安全打开持久化插件索引"),
    };
    ensure!(
        checked_regular_file(&file)?.len() <= MAX_REGISTRY_BYTES,
        "持久化插件索引超过 1 MiB 限额"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_REGISTRY_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_REGISTRY_BYTES,
        "持久化插件索引在读取时超过限额"
    );
    let parsed: InstalledPluginsFile =
        serde_json::from_slice(&bytes).context("持久化插件索引损坏；原文件未修改")?;
    validate_registry_file(&parsed)?;
    Ok(parsed)
}

fn write_registry_file_with(
    root: &Path,
    update: impl FnOnce(&InstalledPluginsFile) -> Result<InstalledPluginsFile>,
) -> Result<()> {
    validate_root_path(root)?;
    ensure_private_dir(root)?;
    let registry_lock = acquire_registry_lock(root)?;
    write_registry_file_locked(root, &registry_lock, update)
}

struct RegistryLockGuard {
    #[cfg(unix)]
    directory: File,
    #[cfg(unix)]
    dir_meta: fs::Metadata,
    _lock: File,
}

fn acquire_registry_lock(root: &Path) -> Result<RegistryLockGuard> {
    #[cfg(unix)]
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root)?;
    #[cfg(unix)]
    let dir_meta = directory.metadata()?;
    #[cfg(unix)]
    ensure!(
        dir_meta.uid() == unsafe { libc::geteuid() }
            && dir_meta.mode() & 0o002 == 0
            && (dir_meta.mode() & 0o020 == 0 || dir_meta.gid() == unsafe { libc::getegid() }),
        "持久化插件目录必须由当前用户拥有且不可被其他用户写入"
    );

    let lock_path = root.join("installed.lock");
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(&lock_path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "持久化插件锁文件无效"
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
    let lock_meta = checked_regular_file(&lock)?;
    crate::fs_atomic::try_lock_with_backoff(&lock, "持久化插件索引")?;
    let vis_lock = fs::symlink_metadata(&lock_path)?;
    #[cfg(unix)]
    ensure!(
        vis_lock.dev() == lock_meta.dev() && vis_lock.ino() == lock_meta.ino(),
        "持久化插件锁文件已替换；未保存"
    );
    #[cfg(not(unix))]
    ensure!(
        vis_lock.is_file() && vis_lock.len() == lock_meta.len(),
        "持久化插件锁文件已替换；未保存"
    );
    Ok(RegistryLockGuard {
        #[cfg(unix)]
        directory,
        #[cfg(unix)]
        dir_meta,
        _lock: lock,
    })
}

fn write_registry_file_locked(
    root: &Path,
    guard: &RegistryLockGuard,
    update: impl FnOnce(&InstalledPluginsFile) -> Result<InstalledPluginsFile>,
) -> Result<()> {
    #[cfg(not(unix))]
    let _ = guard;
    let current = read_registry_file(root)?;
    let next = update(&current)?;
    validate_registry_file(&next)?;
    let encoded = serde_json::to_vec_pretty(&next)?;
    ensure!(
        encoded.len() as u64 <= MAX_REGISTRY_BYTES,
        "持久化插件索引编码超过 1 MiB 限额"
    );

    let path = root.join("installed.json");
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    #[cfg(unix)]
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o600))?;
    temporary.write_all(&encoded)?;
    temporary.as_file().sync_all()?;

    #[cfg(unix)]
    {
        let vis_dir = fs::symlink_metadata(root)?;
        ensure!(
            vis_dir.dev() == guard.dir_meta.dev() && vis_dir.ino() == guard.dir_meta.ino(),
            "持久化插件目录已替换；未保存"
        );
    }
    ensure!(
        read_registry_file(root)? == current,
        "持久化插件索引在保存期间被外部修改；未覆盖"
    );
    temporary.persist(&path).map_err(|err| err.error)?;
    #[cfg(unix)]
    guard
        .directory
        .sync_all()
        .context("持久化插件索引已写入但目录同步失败")?;
    Ok(())
}

fn copy_dir_no_symlinks(
    src_dir: &Path,
    dst_dir: &Path,
    total_files: &mut usize,
    total_bytes: &mut u64,
) -> Result<()> {
    let entries = fs::read_dir(src_dir).context("无法读取插件源目录")?;
    for item in entries {
        let item = item.context("读取插件源目录条目失败")?;
        let name = item.file_name();
        let src_path = item.path();
        let dst_path = dst_dir.join(&name);
        let meta = fs::symlink_metadata(&src_path).context("无法读取插件文件元数据")?;
        ensure!(
            !meta.file_type().is_symlink(),
            "插件包内不得包含符号链接：{}",
            src_path.display()
        );
        if meta.is_dir() {
            ensure_private_dir(&dst_path)?;
            copy_dir_no_symlinks(&src_path, &dst_path, total_files, total_bytes)?;
        } else if meta.is_file() {
            #[cfg(unix)]
            ensure!(
                meta.nlink() == 1,
                "插件包内不得包含硬链接文件：{}",
                src_path.display()
            );
            *total_files += 1;
            ensure!(
                *total_files <= MAX_INSTALL_FILES,
                "持久化插件包内文件数量超过 1024 个上限"
            );
            let file_len = meta.len();
            *total_bytes = total_bytes
                .checked_add(file_len)
                .context("持久化插件包体积计算溢出")?;
            ensure!(
                *total_bytes <= MAX_INSTALL_BYTES,
                "持久化插件包解包后总大小超过 256 MiB 上限"
            );
            let mut open_src = OpenOptions::new();
            open_src.read(true);
            #[cfg(unix)]
            open_src.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            let mut src_file = open_src.open(&src_path).context("无法打开插件源文件")?;
            let actual_meta = src_file.metadata()?;
            ensure!(actual_meta.is_file(), "插件源条目不是普通文件");
            let mut dst_opts = OpenOptions::new();
            dst_opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                let mode = if actual_meta.permissions().mode() & 0o111 != 0 {
                    0o700
                } else {
                    0o600
                };
                dst_opts.mode(mode).custom_flags(libc::O_NOFOLLOW);
            }
            let mut dst_file = dst_opts
                .open(&dst_path)
                .context("无法创建持久化插件目标文件")?;
            let copied = std::io::copy(&mut src_file, &mut dst_file)?;
            ensure!(copied == file_len, "持久化插件文件在复制期间发生变化");
            dst_file.sync_all()?;
        } else {
            anyhow::bail!("插件包内包含不支持的特殊文件类型：{}", src_path.display());
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<[u8; 32]> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = opts.open(path).context("无法打开插件入口校验文件")?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}
