#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt as _, PermissionsExt};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use mio_proxy::MitmAuthority;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct StoredAuthority {
    certificate: Vec<u8>,
    private_key: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthorityStamp {
    file_len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    pub(crate) cert_sha256: [u8; 32],
}

impl AuthorityStamp {
    fn from_metadata(meta: &fs::Metadata, cert_der: &[u8]) -> Self {
        Self {
            file_len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            dev: meta.dev(),
            #[cfg(unix)]
            ino: meta.ino(),
            cert_sha256: Sha256::digest(cert_der).into(),
        }
    }

    fn matches_metadata(&self, meta: &fs::Metadata) -> bool {
        #[cfg(unix)]
        {
            self.file_len == meta.len()
                && self.modified == meta.modified().ok()
                && self.dev == meta.dev()
                && self.ino == meta.ino()
        }
        #[cfg(not(unix))]
        {
            self.file_len == meta.len() && self.modified == meta.modified().ok()
        }
    }
}

pub(crate) fn resolved_identity_dir(preferences_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_IDENTITY_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(custom));
    }
    if let Some(prefs) = preferences_path
        && let Ok(default_cfg_dir) =
            crate::fs_atomic::platform_app_dir(crate::fs_atomic::AppDirKind::Config)
        && prefs.parent() != Some(default_cfg_dir.as_path())
        && let Some(parent) = prefs.parent()
    {
        return Ok(parent.join("identity"));
    }
    default_identity_dir()
}

pub(crate) fn file_changed_on_disk_at(
    directory: &Path,
    current: Option<&AuthorityStamp>,
) -> bool {
    let Some(cur) = current else {
        return false;
    };
    let path = directory.join("authority.json");
    match fs::symlink_metadata(path) {
        Ok(meta) => !cur.matches_metadata(&meta),
        Err(_) => false,
    }
}

pub(crate) fn load_if_changed_at(
    directory: &Path,
    current: Option<&AuthorityStamp>,
) -> Result<Option<(MitmAuthority, AuthorityStamp)>> {
    if let Some(cur) = current {
        let path = directory.join("authority.json");
        if let Ok(meta) = fs::symlink_metadata(&path)
            && cur.matches_metadata(&meta)
        {
            return Ok(None);
        }
    }
    let (authority, stamp) = load_at_with_stamp(directory)?;
    Ok(Some((authority, stamp)))
}

pub(crate) fn fingerprint_sha256(cert_der: &[u8]) -> String {
    let digest = Sha256::digest(cert_der);
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub(crate) fn regenerate_with_stamp_at(
    directory: &Path,
) -> Result<(MitmAuthority, AuthorityStamp)> {
    let authority = MitmAuthority::generate()?;
    let stamp = save_at(directory, &authority)?;
    Ok((authority, stamp))
}

pub(crate) fn save_authority_with_stamp_at(
    directory: &Path,
    authority: &MitmAuthority,
) -> Result<AuthorityStamp> {
    save_at(directory, authority)
}

fn ensure_identity_dir(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(directory)?;
    }
    let metadata = fs::symlink_metadata(directory)?;
    #[cfg(unix)]
    ensure!(
        metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
        "CA 目录必须是仅当前用户可访问的真实目录"
    );
    #[cfg(not(unix))]
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "CA 目录必须是仅当前用户可访问的真实目录"
    );
    Ok(())
}

fn save_at(directory: &Path, authority: &MitmAuthority) -> Result<AuthorityStamp> {
    ensure_identity_dir(directory)?;
    let _lock = crate::fs_atomic::acquire_sidecar_lock(directory, "authority.lock", "CA 密钥")?;
    let path = directory.join("authority.json");
    let cert_der = authority.certificate_der();
    let stored = StoredAuthority {
        certificate: cert_der.clone(),
        private_key: authority.private_key_der(),
    };
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    {
        fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec(&stored)?)?;
    file.as_file().sync_all()?;
    file.persist(&path).map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    let meta = fs::symlink_metadata(&path)?;
    Ok(AuthorityStamp::from_metadata(&meta, &cert_der))
}

pub fn default_identity_dir() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_IDENTITY_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(custom));
    }
    Ok(crate::fs_atomic::platform_app_dir(crate::fs_atomic::AppDirKind::Data)?.join("identity"))
}

fn read_existing_at(path: &Path) -> Result<(MitmAuthority, AuthorityStamp)> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    ensure!(
        metadata.is_file()
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.len() <= 64 * 1024,
        "CA 密钥文件权限或大小异常；未覆盖原密钥"
    );
    #[cfg(not(unix))]
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 64 * 1024,
        "CA 密钥文件权限或大小异常；未覆盖原密钥"
    );
    let stored: StoredAuthority =
        serde_json::from_slice(&fs::read(path)?).context("CA 文件损坏；未生成替代密钥")?;
    let cert_der = stored.certificate.clone();
    let authority = MitmAuthority::from_der(stored.certificate, stored.private_key)?;
    Ok((authority, AuthorityStamp::from_metadata(&metadata, &cert_der)))
}

#[cfg(test)]
fn load_at(directory: &Path) -> Result<MitmAuthority> {
    load_at_with_stamp(directory).map(|(authority, _)| authority)
}

fn load_at_with_stamp(directory: &Path) -> Result<(MitmAuthority, AuthorityStamp)> {
    ensure_identity_dir(directory)?;
    let path = directory.join("authority.json");
    if path.exists() {
        return read_existing_at(&path);
    }
    let _lock = crate::fs_atomic::acquire_sidecar_lock(directory, "authority.lock", "CA 密钥")?;
    if path.exists() {
        return read_existing_at(&path);
    }
    let authority = MitmAuthority::generate()?;
    let cert_der = authority.certificate_der();
    let stored = StoredAuthority {
        certificate: cert_der.clone(),
        private_key: authority.private_key_der(),
    };
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    {
        fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec(&stored)?)?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(&path) {
        Ok(_) => {
            #[cfg(unix)]
            fs::File::open(directory)?.sync_all()?;
            let meta = fs::symlink_metadata(&path)?;
            Ok((authority, AuthorityStamp::from_metadata(&meta, &cert_der)))
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_existing_at(&path)
        }
        Err(error) => Err(error.error.into()),
    }
}

pub(crate) fn export(certificate: &[u8], path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(certificate)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_private_persistent_and_never_silently_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let identity = directory.path().join("identity");
        let first = load_at(&identity).unwrap();
        let second = load_at(&identity).unwrap();
        assert_eq!(first.certificate_der(), second.certificate_der());
        let path = identity.join("authority.json");
        #[cfg(unix)]
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        fs::write(&path, b"damaged").unwrap();
        assert!(load_at(&identity).is_err());
        assert_eq!(fs::read(path).unwrap(), b"damaged");
    }

    #[test]
    fn concurrent_first_load_at_converges_on_identical_ca() {
        let directory = tempfile::tempdir().unwrap();
        let identity = directory.path().join("identity");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let dir = identity.clone();
            let bar = barrier.clone();
            handles.push(std::thread::spawn(move || {
                bar.wait();
                load_at(&dir).unwrap().certificate_der()
            }));
        }
        let certs: Vec<Vec<u8>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for cert in &certs[1..] {
            assert_eq!(cert, &certs[0]);
        }
    }

    #[test]
    fn load_if_changed_detects_disk_replacement_and_skips_unchanged_reads() {
        let directory = tempfile::tempdir().unwrap();
        let identity = directory.path().join("identity");
        let (first, stamp1) = load_at_with_stamp(&identity).unwrap();
        assert!(
            load_if_changed_at(&identity, Some(&stamp1))
                .unwrap()
                .is_none()
        );

        let second = MitmAuthority::generate().unwrap();
        let stamp2 = save_at(&identity, &second).unwrap();
        assert_ne!(stamp1.cert_sha256, stamp2.cert_sha256);

        let (reloaded, reloaded_stamp) = load_if_changed_at(&identity, Some(&stamp1))
            .unwrap()
            .expect("replaced CA must trigger reload");
        assert_eq!(reloaded.certificate_der(), second.certificate_der());
        assert_ne!(reloaded.certificate_der(), first.certificate_der());
        assert_eq!(reloaded_stamp.cert_sha256, stamp2.cert_sha256);
    }
}
