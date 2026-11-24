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

const MAX_AUTHORITY_FILE_BYTES: u64 = 256 * 1024;

#[derive(Serialize, Deserialize)]
struct StoredAuthority {
    certificate: Vec<u8>,
    private_key: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    extra_certificates: Vec<Vec<u8>>,
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
    fn from_metadata(meta: &fs::Metadata, cert_chain: &[Vec<u8>]) -> Self {
        let mut hasher = Sha256::new();
        for cert_der in cert_chain {
            hasher.update(cert_der);
        }
        Self {
            file_len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            dev: meta.dev(),
            #[cfg(unix)]
            ino: meta.ino(),
            cert_sha256: hasher.finalize().into(),
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
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.permissions().mode() & 0o077 == 0,
        "CA 目录必须是仅当前用户可访问的真实目录"
    );
    #[cfg(not(unix))]
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "CA 目录必须是仅当前用户可访问的真实目录"
    );
    Ok(())
}

fn stored_authority(authority: &MitmAuthority) -> Result<(StoredAuthority, Vec<Vec<u8>>)> {
    let mut chain = authority.certificate_chain_der();
    ensure!(!chain.is_empty(), "CA 证书链不能为空");
    let certificate = chain[0].clone();
    let extra_certificates = if chain.len() > 1 {
        chain.split_off(1)
    } else {
        Vec::new()
    };
    let mut full_chain = Vec::with_capacity(1 + extra_certificates.len());
    full_chain.push(certificate.clone());
    full_chain.extend(extra_certificates.iter().cloned());
    Ok((
        StoredAuthority {
            certificate,
            private_key: authority.private_key_der(),
            extra_certificates,
        },
        full_chain,
    ))
}

fn save_at(directory: &Path, authority: &MitmAuthority) -> Result<AuthorityStamp> {
    ensure_identity_dir(directory)?;
    let _lock = crate::fs_atomic::acquire_sidecar_lock(directory, "authority.lock", "CA 密钥")?;
    let path = directory.join("authority.json");
    let (stored, cert_chain) = stored_authority(authority)?;
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
    Ok(AuthorityStamp::from_metadata(&meta, &cert_chain))
}

pub fn default_identity_dir() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_IDENTITY_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(custom));
    }
    Ok(crate::fs_atomic::platform_app_dir(crate::fs_atomic::AppDirKind::Data)?.join("identity"))
}

fn read_existing_at(path: &Path) -> Result<(MitmAuthority, AuthorityStamp)> {
    use std::io::Read as _;

    #[cfg(not(unix))]
    if let Ok(pre_meta) = fs::symlink_metadata(path) {
        ensure!(
            pre_meta.is_file()
                && !pre_meta.file_type().is_symlink()
                && pre_meta.len() <= MAX_AUTHORITY_FILE_BYTES,
            "CA 密钥文件权限或大小异常；未覆盖原密钥"
        );
    }
    let mut open_opts = fs::OpenOptions::new();
    open_opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        open_opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = open_opts.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.len() <= MAX_AUTHORITY_FILE_BYTES,
        "CA 密钥文件权限或大小异常；未覆盖原密钥"
    );
    #[cfg(not(unix))]
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_AUTHORITY_FILE_BYTES,
        "CA 密钥文件权限或大小异常；未覆盖原密钥"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_AUTHORITY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_AUTHORITY_FILE_BYTES,
        "CA 密钥文件权限或大小异常；未覆盖原密钥"
    );
    let stored: StoredAuthority =
        serde_json::from_slice(&bytes).context("CA 文件损坏；未生成替代密钥")?;
    let mut cert_chain = Vec::with_capacity(1 + stored.extra_certificates.len());
    cert_chain.push(stored.certificate);
    cert_chain.extend(stored.extra_certificates);
    let authority = MitmAuthority::from_der_chain(cert_chain.clone(), stored.private_key)?;
    Ok((authority, AuthorityStamp::from_metadata(&metadata, &cert_chain)))
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
    let (stored, cert_chain) = stored_authority(&authority)?;
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
            Ok((authority, AuthorityStamp::from_metadata(&meta, &cert_chain)))
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_existing_at(&path)
        }
        Err(error) => Err(error.error.into()),
    }
}

pub(crate) fn export(certificate: &[u8], path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            !meta.file_type().is_symlink() && meta.is_file(),
            "CA 导出目标不能是符号链接或目录"
        );
    }
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

    #[test]
    fn save_and_reload_preserves_multi_cert_intermediate_ca_chain() {
        let mut root_params = rcgen::CertificateParams::default();
        root_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        root_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        root_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "MIO Test Root CA");
        let root_key = rcgen::KeyPair::generate().unwrap();
        let root_cert = root_params.self_signed(&root_key).unwrap();
        let root_issuer = rcgen::Issuer::new(root_params, root_key);

        let mut inter_params = rcgen::CertificateParams::default();
        inter_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
        inter_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        inter_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "MIO Test Intermediate CA");
        let inter_key = rcgen::KeyPair::generate().unwrap();
        let inter_cert = inter_params.signed_by(&inter_key, &root_issuer).unwrap();

        let directory = tempfile::tempdir().unwrap();
        let identity = directory.path().join("identity");
        let combined_pem = format!("{}{}", inter_cert.pem(), root_cert.pem());
        let chained =
            MitmAuthority::from_pem_pair(&combined_pem, &inter_key.serialize_pem()).unwrap();
        let expected_chain = vec![
            inter_cert.der().as_ref().to_vec(),
            root_cert.der().as_ref().to_vec(),
        ];
        assert_eq!(chained.certificate_chain_der(), expected_chain);

        let stamp = save_authority_with_stamp_at(&identity, &chained).unwrap();
        let (reloaded, reloaded_stamp) = load_at_with_stamp(&identity).unwrap();
        assert_eq!(reloaded.certificate_chain_der(), expected_chain);
        assert_eq!(reloaded_stamp.cert_sha256, stamp.cert_sha256);
    }
}
