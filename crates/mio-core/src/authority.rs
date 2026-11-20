#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
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

pub(crate) fn load_or_create() -> Result<MitmAuthority> {
    load_at(&default_identity_dir()?)
}

pub(crate) fn fingerprint_sha256(cert_der: &[u8]) -> String {
    let digest = Sha256::digest(cert_der);
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub(crate) fn regenerate() -> Result<MitmAuthority> {
    let authority = MitmAuthority::generate()?;
    save_at(&default_identity_dir()?, &authority)?;
    Ok(authority)
}

pub(crate) fn save_authority(authority: &MitmAuthority) -> Result<()> {
    save_at(&default_identity_dir()?, authority)
}

fn save_at(directory: &Path, authority: &MitmAuthority) -> Result<()> {
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
    let path = directory.join("authority.json");
    let stored = StoredAuthority {
        certificate: authority.certificate_der(),
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
    Ok(())
}

pub fn default_identity_dir() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_IDENTITY_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(custom));
    }
    #[cfg(target_os = "macos")]
    {
        let user_directory = std::env::var_os("HOME").context("无法确定用户目录")?;
        Ok(Path::new(&user_directory).join("Library/Application Support/MIO/identity"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            let p = PathBuf::from(xdg);
            if p.is_absolute() {
                return Ok(p.join("mio/identity"));
            }
        }
        let user_directory = std::env::var_os("HOME").context("无法确定用户目录")?;
        Ok(Path::new(&user_directory).join(".local/share/mio/identity"))
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(appdata).join("MIO").join("identity"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .filter(|v| !v.is_empty())
        {
            return Ok(PathBuf::from(profile)
                .join("AppData")
                .join("Roaming")
                .join("MIO")
                .join("identity"));
        }
        anyhow::bail!("无法确定用户目录")
    }
}

fn load_at(directory: &Path) -> Result<MitmAuthority> {
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
    let path = directory.join("authority.json");
    if path.exists() {
        let metadata = fs::symlink_metadata(&path)?;
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
            serde_json::from_slice(&fs::read(&path)?).context("CA 文件损坏；未生成替代密钥")?;
        return Ok(MitmAuthority::from_der(
            stored.certificate,
            stored.private_key,
        )?);
    }
    let authority = MitmAuthority::generate()?;
    let stored = StoredAuthority {
        certificate: authority.certificate_der(),
        private_key: authority.private_key_der(),
    };
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(&serde_json::to_vec(&stored)?)?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(&path) {
        Ok(_) => {
            #[cfg(unix)]
            fs::File::open(directory)?.sync_all()?;
            Ok(authority)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => load_at(directory),
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
}
