#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use mio_plugin_api::protocol::{MAX_STORAGE_KEY_BYTES, MAX_STORAGE_VALUE_BYTES};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

const VERSION: u32 = 1;
const MAX_KEYS_PER_PLUGIN: usize = 64;
const MAX_BYTES_PER_PLUGIN: usize = 256 * 1024;
const MAX_PLUGINS: usize = 64;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;

type PluginPrefsMap = BTreeMap<String, BTreeMap<String, Vec<u8>>>;

mod base64_plugin_map {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::PluginPrefsMap;

    pub fn serialize<S>(plugins: &PluginPrefsMap, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded: BTreeMap<&str, BTreeMap<&str, String>> = plugins
            .iter()
            .map(|(plugin_id, entries)| {
                let map = entries
                    .iter()
                    .map(|(k, v)| (k.as_str(), crate::codec_util::base64_encode(v)))
                    .collect();
                (plugin_id.as_str(), map)
            })
            .collect();
        encoded.serialize(serializer)
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum EncodedPrefValue {
        Base64(String),
        LegacyBytes(Vec<u8>),
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PluginPrefsMap, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = BTreeMap::<String, BTreeMap<String, EncodedPrefValue>>::deserialize(deserializer)?;
        let mut out = BTreeMap::new();
        for (plugin_id, entries) in raw {
            let mut decoded_entries = BTreeMap::new();
            for (k, v) in entries {
                let bytes = match v {
                    EncodedPrefValue::Base64(s) => crate::codec_util::base64_decode(s.as_bytes())
                        .map_err(serde::de::Error::custom)?,
                    EncodedPrefValue::LegacyBytes(b) => b,
                };
                decoded_entries.insert(k, bytes);
            }
            out.insert(plugin_id, decoded_entries);
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedPluginPrefs {
    version: u32,
    #[serde(default, with = "base64_plugin_map")]
    plugins: PluginPrefsMap,
}

impl Default for PersistedPluginPrefs {
    fn default() -> Self {
        Self {
            version: VERSION,
            plugins: BTreeMap::new(),
        }
    }
}

pub fn default_plugin_preferences_path() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_PLUGIN_PREFERENCES_PATH") {
        let path = PathBuf::from(custom);
        validate_path(&path)?;
        return Ok(path);
    }
    let user_prefs = crate::preferences::default_user_preferences_path()?;
    let parent = user_prefs.parent().context("用户偏好路径缺少父目录")?;
    Ok(parent.join("plugin-preferences.json"))
}

pub struct PluginPreferencesStore {
    path: Option<PathBuf>,
    memory: PluginPrefsMap,
}

impl Default for PluginPreferencesStore {
    fn default() -> Self {
        Self::new(None)
    }
}

impl PluginPreferencesStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            memory: BTreeMap::new(),
        }
    }

    pub fn from_env_or_memory() -> Self {
        let path = std::env::var_os("MIO_PLUGIN_PREFERENCES_PATH")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self::new(path)
    }

    pub fn from_env_or_user_preferences(user_preferences_path: Option<&Path>) -> Self {
        if std::env::var_os("MIO_PLUGIN_PREFERENCES_PATH").is_some_and(|v| !v.is_empty()) {
            return Self::from_env_or_memory();
        }
        Self::new(user_preferences_path.map(|p| p.with_file_name("plugin-preferences.json")))
    }

    #[cfg(test)]
    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
    }

    pub fn get(&self, plugin_id: &str, key: &str) -> Result<Option<Vec<u8>>> {
        validate_plugin_and_key(plugin_id, key)?;
        if let Some(path) = &self.path {
            let data = read_file(path)?;
            return Ok(data
                .plugins
                .get(plugin_id)
                .and_then(|ns| ns.get(key))
                .cloned());
        }
        Ok(self
            .memory
            .get(plugin_id)
            .and_then(|ns| ns.get(key))
            .cloned())
    }

    pub fn set(
        &mut self,
        plugin_id: &str,
        key: &str,
        value: &[u8],
        cancel: &CancellationToken,
    ) -> Result<()> {
        ensure!(!cancel.is_cancelled(), "插件偏好写入已取消");
        validate_plugin_and_key(plugin_id, key)?;
        ensure!(
            value.len() <= MAX_STORAGE_VALUE_BYTES,
            "插件偏好单值超过 64 KiB 限额"
        );
        if let Some(path) = self.path.clone() {
            write_file_with(&path, cancel, |current| {
                let mut next = current.clone();
                apply_mutation(&mut next.plugins, plugin_id, key, value)?;
                Ok(next)
            })?;
            return Ok(());
        }
        apply_mutation(&mut self.memory, plugin_id, key, value)
    }
}

fn validate_plugin_and_key(plugin_id: &str, key: &str) -> Result<()> {
    ensure!(
        !plugin_id.is_empty()
            && plugin_id.len() <= 128
            && plugin_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')),
        "插件 ID 无效"
    );
    ensure!(
        !key.is_empty() && key.len() <= MAX_STORAGE_KEY_BYTES,
        "插件偏好 key 为空或超过 256 字节限额"
    );
    Ok(())
}

fn apply_mutation(
    plugins: &mut BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    plugin_id: &str,
    key: &str,
    value: &[u8],
) -> Result<()> {
    if value.is_empty() {
        if let Some(ns) = plugins.get_mut(plugin_id) {
            ns.remove(key);
            if ns.is_empty() {
                plugins.remove(plugin_id);
            }
        }
        return Ok(());
    }
    if !plugins.contains_key(plugin_id) {
        ensure!(
            plugins.len() < MAX_PLUGINS,
            "跨项目插件偏好命名空间达到 64 个上限"
        );
    }
    let mut candidate = plugins.get(plugin_id).cloned().unwrap_or_default();
    if !candidate.contains_key(key) {
        ensure!(
            candidate.len() < MAX_KEYS_PER_PLUGIN,
            "单插件跨项目偏好键数超过 64 个限额"
        );
    }
    candidate.insert(key.to_owned(), value.to_vec());
    let total_bytes: usize = candidate.iter().map(|(k, v)| k.len() + v.len()).sum();
    ensure!(
        total_bytes <= MAX_BYTES_PER_PLUGIN,
        "单插件跨项目偏好总容量超过 256 KiB 限额"
    );
    plugins.insert(plugin_id.to_owned(), candidate);
    Ok(())
}

fn validate_loaded(data: &PersistedPluginPrefs) -> Result<()> {
    ensure!(
        data.version == VERSION,
        "不支持的跨项目插件偏好版本；原文件未修改"
    );
    ensure!(
        data.plugins.len() <= MAX_PLUGINS,
        "跨项目插件偏好命名空间数量超限"
    );
    for (plugin_id, entries) in &data.plugins {
        ensure!(
            entries.len() <= MAX_KEYS_PER_PLUGIN,
            "单插件跨项目偏好键数超过 64 个限额"
        );
        let mut total = 0usize;
        for (k, v) in entries {
            validate_plugin_and_key(plugin_id, k)?;
            ensure!(
                v.len() <= MAX_STORAGE_VALUE_BYTES,
                "跨项目插件偏好单值超过 64 KiB 限额"
            );
            total = total.saturating_add(k.len()).saturating_add(v.len());
        }
        ensure!(
            total <= MAX_BYTES_PER_PLUGIN,
            "单插件跨项目偏好总容量超过 256 KiB 限额"
        );
    }
    Ok(())
}

fn validate_path(path: &Path) -> Result<PathBuf> {
    let raw = path.as_os_str().as_encoded_bytes();
    ensure!(
        !raw.is_empty() && raw.len() <= MAX_PATH_BYTES && !raw.contains(&0),
        "插件偏好路径无效"
    );
    ensure!(
        path.is_absolute() || raw.starts_with(b"/"),
        "插件偏好路径必须是绝对路径"
    );
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "插件偏好路径不能包含父目录跳转"
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .context("插件偏好路径缺少父目录")?;
    for ancestor in parent.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "插件偏好目录不得经过符号链接或特殊文件"
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).context("无法检查插件偏好目录"),
        }
    }
    Ok(parent.to_owned())
}

fn checked_file(file: &File) -> Result<fs::Metadata> {
    let meta = file.metadata()?;
    #[cfg(unix)]
    ensure!(
        meta.is_file()
            && meta.nlink() == 1
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o002 == 0
            && (meta.mode() & 0o020 == 0 || meta.gid() == unsafe { libc::getegid() }),
        "插件偏好必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    #[cfg(not(unix))]
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "插件偏好必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    Ok(meta)
}

fn read_file(path: &Path) -> Result<PersistedPluginPrefs> {
    validate_path(path)?;
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "无法安全打开插件偏好文件"
        );
    }
    let mut open_opts = OpenOptions::new();
    open_opts.read(true);
    #[cfg(unix)]
    open_opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = match open_opts.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PersistedPluginPrefs::default());
        }
        Err(err) => return Err(err).context("无法安全打开插件偏好文件"),
    };
    ensure!(
        checked_file(&file)?.len() <= MAX_FILE_BYTES,
        "插件偏好文件超过读取上限"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "插件偏好文件在读取时超过上限"
    );
    let parsed: PersistedPluginPrefs =
        serde_json::from_slice(&bytes).context("插件偏好文件损坏；原文件未修改")?;
    validate_loaded(&parsed)?;
    Ok(parsed)
}

fn write_file_with(
    path: &Path,
    cancel: &CancellationToken,
    update: impl FnOnce(&PersistedPluginPrefs) -> Result<PersistedPluginPrefs>,
) -> Result<()> {
    let parent = validate_path(path)?;
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&parent)?;
    validate_path(path)?;
    #[cfg(unix)]
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&parent)?;
    #[cfg(unix)]
    let dir_meta = directory.metadata()?;
    #[cfg(unix)]
    ensure!(
        dir_meta.uid() == unsafe { libc::geteuid() }
            && dir_meta.mode() & 0o002 == 0
            && (dir_meta.mode() & 0o020 == 0 || dir_meta.gid() == unsafe { libc::getegid() }),
        "插件偏好目录必须由当前用户拥有且不可被其他用户写入"
    );
    let lock_path = parent.join("plugin-preferences.lock");
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(&lock_path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "插件偏好锁文件无效"
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
    let lock_meta = checked_file(&lock)?;
    crate::fs_atomic::try_lock_with_backoff(&lock, "插件偏好")?;
    let vis_lock = fs::symlink_metadata(&lock_path)?;
    #[cfg(unix)]
    ensure!(
        vis_lock.dev() == lock_meta.dev() && vis_lock.ino() == lock_meta.ino(),
        "插件偏好锁文件已替换；未保存"
    );
    #[cfg(not(unix))]
    ensure!(
        vis_lock.is_file() && vis_lock.len() == lock_meta.len(),
        "插件偏好锁文件已替换；未保存"
    );
    let current = read_file(path)?;
    let next = update(&current)?;
    validate_loaded(&next)?;
    let encoded = serde_json::to_vec(&next)?;
    ensure!(
        encoded.len() as u64 <= MAX_FILE_BYTES,
        "插件偏好编码大小超限"
    );
    ensure!(!cancel.is_cancelled(), "插件偏好写入已取消");
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    temporary.write_all(&encoded)?;
    temporary.as_file().sync_all()?;
    validate_path(path)?;
    #[cfg(unix)]
    {
        let vis_dir = fs::symlink_metadata(&parent)?;
        ensure!(
            vis_dir.dev() == dir_meta.dev() && vis_dir.ino() == dir_meta.ino(),
            "插件偏好目录已替换；未保存"
        );
    }
    ensure!(
        read_file(path)? == current,
        "插件偏好文件已在保存期间改变；未覆盖"
    );
    temporary.persist(path).map_err(|err| err.error)?;
    #[cfg(unix)]
    directory
        .sync_all()
        .context("插件偏好已发布但目录同步失败")?;
    Ok(())
}
