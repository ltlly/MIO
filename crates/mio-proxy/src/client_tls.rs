use std::{fmt, time::Duration};

use rama::crypto::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};

use crate::ProxyError;

mod pem;
mod pkcs12;
mod selection;
#[cfg(test)]
mod tests;

#[cfg(test)]
use pkcs12::PrivateTempFile;
use pkcs12::{ZeroizeBytes, unpack_pkcs12_to_pem};
pub use selection::{
    matches_host, resolve_client_identity, select_client_identity, select_client_rule,
};

pub const MAX_CLIENT_CERT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CLIENT_CERT_PASSWORD_BYTES: usize = 1024;
pub const MAX_HOST_PATTERN_BYTES: usize = 253;

const MAX_CLIENT_CERT_CHAIN_LEN: usize = 32;
const PKCS12_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PKCS12_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PKCS12_STDERR_BYTES: usize = 16 * 1024;

pub(super) fn resolve_openssl_bin() -> std::ffi::OsString {
    if let Some(custom) = std::env::var_os("MIO_OPENSSL_BIN").filter(|v| !v.is_empty()) {
        return custom;
    }
    #[cfg(unix)]
    {
        for candidate in [
            "/usr/bin/openssl",
            "/opt/homebrew/bin/openssl",
            "/usr/local/bin/openssl",
        ] {
            if std::path::Path::new(candidate).is_file() {
                return std::ffi::OsString::from(candidate);
            }
        }
        std::ffi::OsString::from("openssl")
    }
    #[cfg(windows)]
    {
        for candidate in [
            r"C:\Program Files\Git\usr\bin\openssl.exe",
            r"C:\Program Files\OpenSSL-Win64\bin\openssl.exe",
            r"C:\Program Files\OpenSSL\bin\openssl.exe",
        ] {
            if std::path::Path::new(candidate).is_file() {
                return std::ffi::OsString::from(candidate);
            }
        }
        std::ffi::OsString::from("openssl.exe")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientCertificateFormat {
    #[default]
    Pem,
    Pkcs12,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCertificateRule {
    pub enabled: bool,
    pub host_pattern: String,
    pub format: ClientCertificateFormat,
    pub certificate_bytes: Vec<u8>,
    pub private_key_pem: Vec<u8>,
    pub password: String,
}

impl fmt::Debug for ClientCertificateRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientCertificateRule")
            .field("enabled", &self.enabled)
            .field("host_pattern", &self.host_pattern)
            .field("format", &self.format)
            .field("certificate_bytes_len", &self.certificate_bytes.len())
            .field("has_private_key_pem", &!self.private_key_pem.is_empty())
            .field("has_password", &!self.password.is_empty())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCertificateSummary {
    pub subject_hint: String,
    pub cert_count: usize,
}

pub struct ParsedClientIdentity {
    cert_chain: Vec<CertificateDer<'static>>,
    private_key: PrivateKeyDer<'static>,
    summary: ClientCertificateSummary,
}

impl Clone for ParsedClientIdentity {
    fn clone(&self) -> Self {
        Self {
            cert_chain: self.cert_chain.clone(),
            private_key: self.private_key.clone_key(),
            summary: self.summary.clone(),
        }
    }
}

impl fmt::Debug for ParsedClientIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedClientIdentity")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}
