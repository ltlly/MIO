use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub api_version: u32,
    pub engine: String,
    pub entry: PathBuf,
    pub permissions: Vec<Permission>,
    pub contributions: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct VerifiedPackage {
    pub manifest: PluginManifest,
    pub directory: PathBuf,
    pub(crate) work_dir: PathBuf,
    pub executable: PathBuf,
    pub(crate) executable_size: u64,
    pub(crate) modified: std::time::SystemTime,
    pub(crate) manifest_modified: std::time::SystemTime,
    pub(crate) manifest_bytes: Vec<u8>,
    pub(crate) source_archive: Option<super::archive::ArchiveFingerprint>,
    pub(crate) _unpacked_dir: Option<Arc<tempfile::TempDir>>,
}

impl VerifiedPackage {
    /// Logical working root directory for runtime execution and manifest lookup.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Run on a service thread; discovery never executes entry or inherits a project plugin.
    pub fn read(path: &Path) -> Result<Self> {
        let canonical = path.canonicalize().context("插件包路径不存在")?;
        let path_meta = fs::metadata(&canonical).context("插件包路径不存在")?;
        let (directory, work_dir, source_archive, unpacked_dir) = if path_meta.is_file() {
            let (tempdir, unpacked_root, fp) = super::archive::unpack_plugin_archive(&canonical)?;
            (canonical, unpacked_root, Some(fp), Some(tempdir))
        } else if path_meta.is_dir() {
            (canonical.clone(), canonical, None, None)
        } else {
            bail!("插件包必须是目录或单文件归档（.tar.gz / .tgz / .mio-plugin）");
        };
        let manifest_path = work_dir.join("manifest.json");
        let manifest_bytes = read_manifest(&manifest_path)?;
        let manifest_modified = fs::metadata(&manifest_path)?.modified()?;
        let manifest: PluginManifest =
            serde_json::from_slice(&manifest_bytes).context("插件 manifest 无效")?;
        ensure!(
            !manifest.id.is_empty()
                && manifest.id.len() <= 128
                && manifest
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')),
            "插件 id 无效"
        );
        ensure!(
            !manifest.version.is_empty() && manifest.version.len() <= 64,
            "插件 version 无效"
        );
        ensure!(
            manifest.api_version == protocol::PROTOCOL_MAJOR && manifest.engine == "native",
            "只接受兼容的可信 native 插件包"
        );
        ensure!(
            !manifest.entry.as_os_str().is_empty()
                && manifest
                    .entry
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "插件入口必须是包内相对路径"
        );
        let candidate_entry = resolve_entry_path(&work_dir, &manifest.entry, &manifest.engine);
        let executable = candidate_entry.canonicalize().context("插件入口不存在")?;
        ensure!(executable.starts_with(&work_dir), "插件入口不能指向包外");
        #[cfg(not(unix))]
        if let Ok(meta) = fs::symlink_metadata(&executable) {
            ensure!(meta.is_file(), "插件入口不是文件");
        }
        let mut entry_opts = fs::OpenOptions::new();
        entry_opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            entry_opts.custom_flags(libc::O_NONBLOCK);
        }
        let mut entry = entry_opts.open(&executable)?;
        let metadata = entry.metadata()?;
        ensure!(metadata.is_file(), "插件入口不是文件");
        #[cfg(test)]
        super::tests_package::after_entry_metadata();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                metadata.permissions().mode() & 0o111 != 0,
                "插件入口不可执行"
            );
        }
        let mut magic = [0; 4];
        entry.read_exact(&mut magic)?;
        ensure!(
            is_native_executable_magic(magic),
            "native worker 必须是原生可执行程序（Mach-O / ELF / PE）"
        );
        let permissions: BTreeSet<_> = manifest.permissions.iter().copied().collect();
        ensure!(
            permissions.len() == manifest.permissions.len(),
            "manifest 权限重复"
        );
        ensure!(
            manifest.contributions.iter().all(|c| matches!(
                c.as_str(),
                "suite_panel"
                    | "message_editor"
                    | "message_response_editor"
                    | "context_menu"
                    | "http_hook"
                    | "ws_hook"
                    | "session_action"
                    | "payload_generator"
                    | "payload_processor"
                    | "passive_check"
                    | "active_check"
            )),
            "包含当前协议未支持的必需贡献"
        );
        ensure!(
            !manifest.contributions.is_empty(),
            "插件包必须声明至少一个支持的贡献"
        );
        Ok(Self {
            manifest,
            directory,
            work_dir,
            executable,
            executable_size: metadata.len(),
            modified: metadata.modified()?,
            manifest_modified,
            manifest_bytes,
            source_archive,
            _unpacked_dir: unpacked_dir,
        })
    }

    pub(crate) fn unchanged(&self) -> Result<()> {
        if let Some(expected) = self.source_archive {
            let meta = fs::metadata(&self.directory).context("单文件插件归档已不存在")?;
            ensure!(
                meta.is_file()
                    && meta.len() == expected.size
                    && meta.modified()? == expected.modified,
                "插件归档文件已改变，请重新检查并确认本地程序信任"
            );
        }
        ensure!(
            read_manifest(&self.work_dir.join("manifest.json"))? == self.manifest_bytes
                && serde_json::from_slice::<PluginManifest>(&self.manifest_bytes)? == self.manifest,
            "插件 manifest 已改变，请重新检查并确认本地程序信任"
        );
        let current = fs::metadata(&self.executable)?;
        ensure!(
            current.len() == self.executable_size && current.modified()? == self.modified,
            "插件入口已改变，请重新检查并确认本地程序信任"
        );
        ensure!(
            resolve_entry_path(&self.work_dir, &self.manifest.entry, &self.manifest.engine)
                .canonicalize()?
                == self.executable,
            "插件入口目标已改变，请重新确认"
        );
        Ok(())
    }

    pub(crate) fn poll_source_fingerprint(
        &self,
    ) -> Option<(std::time::SystemTime, u64, std::time::SystemTime, u64)> {
        if self.source_archive.is_some() {
            let meta = fs::metadata(&self.directory).ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta.modified().ok()?;
            let size = meta.len();
            return Some((modified, size, modified, size));
        }
        let manifest_path = self.work_dir.join("manifest.json");
        let entry_path = resolve_entry_path(&self.work_dir, &self.manifest.entry, &self.manifest.engine);
        let m_meta = fs::metadata(&manifest_path).ok()?;
        let e_meta = fs::metadata(&entry_path).ok()?;
        Some((
            m_meta.modified().ok()?,
            m_meta.len(),
            e_meta.modified().ok()?,
            e_meta.len(),
        ))
    }
}

pub(super) fn resolve_entry_path(directory: &Path, entry: &Path, engine: &str) -> PathBuf {
    let direct = directory.join(entry);
    #[cfg(windows)]
    if engine == "native" && !direct.exists() && entry.extension().is_none() {
        let with_exe = direct.with_extension("exe");
        if with_exe.exists() {
            return with_exe;
        }
    }
    #[cfg(not(windows))]
    let _ = engine;
    direct
}

fn is_native_executable_magic(magic: [u8; 4]) -> bool {
    let be = u32::from_be_bytes(magic);
    [
        0xfeed_face,
        0xfeed_facf,
        0xcefa_edfe,
        0xcffa_edfe,
        0xcafe_babe,
        0xbeba_feca,
        0xcafe_babf,
        0xbfba_feca,
        0x7f45_4c46, // ELF: \x7fELF
    ]
    .contains(&be)
        || &magic[..2] == b"MZ"
}

pub(crate) fn read_manifest(path: &Path) -> Result<Vec<u8>> {
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(meta.is_file(), "插件 manifest 必须是普通文件");
    }
    // Nonblocking open prevents a replaced FIFO from trapping discovery or shutdown before fstat.
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let file = opts.open(path)?;
    ensure!(file.metadata()?.is_file(), "插件 manifest 必须是普通文件");
    let mut bytes = Vec::new();
    file.take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_MANIFEST_BYTES,
        "插件 manifest 超过 16 KiB"
    );
    Ok(bytes)
}
/// Only construct after an explicit native-program trust UI and per-capability consent.
#[derive(Clone, Debug)]
pub struct NativeTrust {
    pub(super) executable: PathBuf,
    pub(super) executable_size: u64,
    pub(super) modified: std::time::SystemTime,
    pub(super) plugin_id: String,
    pub(super) version: String,
    pub(super) grants: BTreeSet<Permission>,
    pub(super) manifest_bytes: Vec<u8>,
}

impl NativeTrust {
    pub fn confirmed(
        package: &VerifiedPackage,
        grants: impl IntoIterator<Item = Permission>,
    ) -> Result<Self> {
        let grants: BTreeSet<_> = grants.into_iter().collect();
        ensure!(
            grants
                .iter()
                .all(|p| package.manifest.permissions.contains(p)),
            "不能授予 manifest 未申请的能力"
        );
        Ok(Self {
            executable: package.executable.clone(),
            executable_size: package.executable_size,
            modified: package.modified,
            plugin_id: package.manifest.id.clone(),
            version: package.manifest.version.clone(),
            grants,
            manifest_bytes: package.manifest_bytes.clone(),
        })
    }
}
