use crate::ProxyError;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rama::crypto::{
    cert::{
        CertificateAuthorityData, CertificateIdentity, CertificateSubject, CertificateValidity,
        GeneratedServerAuthConfig, LeafCertRequest, SelfSignedCaConfig, generate_server_auth,
    },
    dep::x509_parser::{self, extensions::GeneralName},
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use rama::net::address::Host;
use rama::tls::{
    KeyLogIntent, ProtocolVersion,
    client::{ServerVerifyMode, TlsClientConfig},
    rustls::{
        client::RustlsClientConfigExt,
        dep::rustls::{self, ServerConfig},
        server::{DynamicConfigProvider, RustlsServerConfigExt},
    },
    server::TlsServerConfig,
};
use sha2::{Digest as _, Sha256};
use std::{
    collections::VecDeque,
    fmt::Write as _,
    net::{Ipv4Addr, Ipv6Addr},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const CERTIFICATE_CACHE_LIMIT: usize = 1024;
const CLIENT_TLS_CACHE_LIMIT: usize = 64;
const CERTIFICATE_CACHE_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_CUSTOM_CERT_CHAIN_LEN: usize = 16;
const CA_CERT_NOT_BEFORE_SKEW: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const LEAF_CERT_LIFETIME: Duration = Duration::from_secs(180 * 24 * 60 * 60);
const LEAF_CERT_NOT_BEFORE_SKEW: Duration = Duration::from_secs(14 * 24 * 60 * 60);

pub(crate) fn shared_crypto_provider() -> &'static Arc<rustls::crypto::CryptoProvider> {
    static PROVIDER: OnceLock<Arc<rustls::crypto::CryptoProvider>> = OnceLock::new();
    PROVIDER.get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ListenerTlsProtocols {
    #[default]
    Default,
    Tls13Only,
    Tls12Only,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ListenerCertMode {
    #[default]
    PerHostCaSigned,
    SelfSigned,
    SpecificHostname(String),
    CustomPem {
        cert_pem: String,
        key_pem: String,
    },
}

impl ListenerCertMode {
    pub fn validate(&self) -> Result<(), ProxyError> {
        match self {
            Self::PerHostCaSigned | Self::SelfSigned => Ok(()),
            Self::SpecificHostname(hostname) => {
                let normalized = crate::route::normalize_host(hostname);
                if normalized.is_empty() {
                    return Err(ProxyError::Tls("固定证书主机名不能为空".into()));
                }
                let host: Host = normalized.parse().map_err(ProxyError::tls)?;
                let _ = CertificateIdentity::try_from_host(&host).map_err(ProxyError::tls)?;
                Ok(())
            }
            Self::CustomPem { cert_pem, key_pem } => {
                let (certs, key) = parse_server_pem_pair(cert_pem, key_pem)?;
                let provider = shared_crypto_provider();
                let certified =
                    rustls::sign::CertifiedKey::from_der(certs, key, provider).map_err(|err| {
                        ProxyError::Tls(format!("监听器自定义 PEM 证书与私钥加载失败：{err}"))
                    })?;
                certified.keys_match().map_err(|err| {
                    ProxyError::Tls(format!("监听器自定义 PEM 证书与私钥不匹配：{err}"))
                })?;
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ListenerTlsOptions {
    pub enable_http2: bool,
    pub protocol_filter: ListenerTlsProtocols,
    pub cert_mode: ListenerCertMode,
}

impl Default for ListenerTlsOptions {
    fn default() -> Self {
        Self {
            enable_http2: true,
            protocol_filter: ListenerTlsProtocols::Default,
            cert_mode: ListenerCertMode::PerHostCaSigned,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObservedCertInfo {
    pub host: String,
    pub port: u16,
    pub subject: String,
    pub issuer: String,
    pub valid_from: String,
    pub valid_to: String,
    pub sans: Vec<String>,
    pub sha256_fingerprint: String,
    pub pem: String,
}

pub fn parse_observed_cert(
    host: &str,
    port: u16,
    leaf_der: &[u8],
) -> Result<ObservedCertInfo, ProxyError> {
    let (remaining, parsed) = x509_parser::parse_x509_certificate(leaf_der)
        .map_err(|err| ProxyError::Tls(format!("上游服务器证书无法解析：{err}")))?;
    if !remaining.is_empty() {
        return Err(ProxyError::Tls("上游服务器证书包含额外尾随字节".into()));
    }
    let subject = parsed.subject().to_string();
    let issuer = parsed.issuer().to_string();
    let validity = parsed.validity();
    let valid_from = validity
        .not_before
        .to_rfc2822()
        .unwrap_or_else(|_| validity.not_before.to_string());
    let valid_to = validity
        .not_after
        .to_rfc2822()
        .unwrap_or_else(|_| validity.not_after.to_string());
    let mut sans = Vec::new();
    if let Ok(Some(san_ext)) = parsed.subject_alternative_name() {
        for name in &san_ext.value.general_names {
            match name {
                GeneralName::DNSName(dns) => sans.push((*dns).to_owned()),
                GeneralName::IPAddress(bytes) => {
                    if let Ok(octets) = <[u8; 4]>::try_from(*bytes) {
                        sans.push(Ipv4Addr::from(octets).to_string());
                    } else if let Ok(octets) = <[u8; 16]>::try_from(*bytes) {
                        sans.push(Ipv6Addr::from(octets).to_string());
                    }
                }
                GeneralName::URI(uri) => sans.push((*uri).to_owned()),
                GeneralName::RFC822Name(mail) => sans.push((*mail).to_owned()),
                _ => {}
            }
        }
    }
    let digest = Sha256::digest(leaf_der);
    let mut sha256_fingerprint = String::with_capacity(digest.len() * 3 - 1);
    for (idx, byte) in digest.iter().enumerate() {
        if idx > 0 {
            sha256_fingerprint.push(':');
        }
        let _ = write!(sha256_fingerprint, "{byte:02X}");
    }
    Ok(ObservedCertInfo {
        host: crate::route::normalize_host(host),
        port,
        subject,
        issuer,
        valid_from,
        valid_to,
        sans,
        sha256_fingerprint,
        pem: der_to_pem(leaf_der, "CERTIFICATE"),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TlsVersion {
    Tls12,
    Tls13,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TlsCipherSuiteSpec {
    pub name: &'static str,
    pub protocol: TlsVersion,
}

pub const SUPPORTED_TLS_CIPHER_SUITES: &[TlsCipherSuiteSpec] = &[
    TlsCipherSuiteSpec {
        name: "TLS13_AES_256_GCM_SHA384",
        protocol: TlsVersion::Tls13,
    },
    TlsCipherSuiteSpec {
        name: "TLS13_AES_128_GCM_SHA256",
        protocol: TlsVersion::Tls13,
    },
    TlsCipherSuiteSpec {
        name: "TLS13_CHACHA20_POLY1305_SHA256",
        protocol: TlsVersion::Tls13,
    },
    TlsCipherSuiteSpec {
        name: "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
        protocol: TlsVersion::Tls12,
    },
    TlsCipherSuiteSpec {
        name: "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
        protocol: TlsVersion::Tls12,
    },
    TlsCipherSuiteSpec {
        name: "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
        protocol: TlsVersion::Tls12,
    },
    TlsCipherSuiteSpec {
        name: "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
        protocol: TlsVersion::Tls12,
    },
    TlsCipherSuiteSpec {
        name: "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
        protocol: TlsVersion::Tls12,
    },
    TlsCipherSuiteSpec {
        name: "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
        protocol: TlsVersion::Tls12,
    },
];

fn resolve_ring_cipher_suite(
    name: &str,
) -> Option<(TlsVersion, rustls::SupportedCipherSuite)> {
    use rustls::crypto::ring::cipher_suite;
    match name {
        "TLS13_AES_256_GCM_SHA384" => {
            Some((TlsVersion::Tls13, cipher_suite::TLS13_AES_256_GCM_SHA384))
        }
        "TLS13_AES_128_GCM_SHA256" => {
            Some((TlsVersion::Tls13, cipher_suite::TLS13_AES_128_GCM_SHA256))
        }
        "TLS13_CHACHA20_POLY1305_SHA256" => Some((
            TlsVersion::Tls13,
            cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
        )),
        "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384" => Some((
            TlsVersion::Tls12,
            cipher_suite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        )),
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256" => Some((
            TlsVersion::Tls12,
            cipher_suite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        )),
        "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256" => Some((
            TlsVersion::Tls12,
            cipher_suite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        )),
        "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384" => Some((
            TlsVersion::Tls12,
            cipher_suite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        )),
        "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256" => Some((
            TlsVersion::Tls12,
            cipher_suite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        )),
        "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256" => Some((
            TlsVersion::Tls12,
            cipher_suite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        )),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UpstreamTlsProtocolMode {
    #[default]
    Default,
    AllSupported,
    Tls12Only,
    Tls13Only,
    Custom,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UpstreamTlsOptions {
    pub disable_session_resume: bool,
    pub protocol_mode: UpstreamTlsProtocolMode,
    pub tls_min_version: Option<TlsVersion>,
    pub tls_max_version: Option<TlsVersion>,
    pub custom_protocols: Vec<TlsVersion>,
    pub custom_cipher_suites: Vec<String>,
}

impl UpstreamTlsOptions {
    pub fn validate(&self) -> Result<(), ProxyError> {
        let versions = self.resolved_tls_versions()?;
        let _ = self.resolved_cipher_suites(versions.as_deref())?;
        Ok(())
    }

    fn resolved_tls_versions(&self) -> Result<Option<Vec<TlsVersion>>, ProxyError> {
        if let (Some(min), Some(max)) = (self.tls_min_version, self.tls_max_version)
            && min > max
        {
            return Err(ProxyError::Tls("TLS 最低版本不能高于最高版本".into()));
        }
        match self.protocol_mode {
            UpstreamTlsProtocolMode::Tls12Only => Ok(Some(vec![TlsVersion::Tls12])),
            UpstreamTlsProtocolMode::Tls13Only => Ok(Some(vec![TlsVersion::Tls13])),
            UpstreamTlsProtocolMode::Custom => {
                if self.custom_protocols.is_empty() {
                    return Err(ProxyError::Tls("自定义 TLS 至少需启用一个协议版本".into()));
                }
                let mut list = Vec::with_capacity(self.custom_protocols.len());
                for proto in &self.custom_protocols {
                    if !list.contains(proto) {
                        list.push(*proto);
                    }
                }
                if let Some(min) = self.tls_min_version {
                    list.retain(|v| *v >= min);
                }
                if let Some(max) = self.tls_max_version {
                    list.retain(|v| *v <= max);
                }
                if list.is_empty() {
                    return Err(ProxyError::Tls("自定义 TLS 至少需启用一个协议版本".into()));
                }
                Ok(Some(list))
            }
            UpstreamTlsProtocolMode::Default | UpstreamTlsProtocolMode::AllSupported => {
                match (self.tls_min_version, self.tls_max_version) {
                    (Some(TlsVersion::Tls13), _) => Ok(Some(vec![TlsVersion::Tls13])),
                    (_, Some(TlsVersion::Tls12)) => Ok(Some(vec![TlsVersion::Tls12])),
                    (Some(TlsVersion::Tls12), Some(TlsVersion::Tls13))
                    | (Some(TlsVersion::Tls12), None)
                    | (None, Some(TlsVersion::Tls13)) => {
                        Ok(Some(vec![TlsVersion::Tls13, TlsVersion::Tls12]))
                    }
                    (None, None) => Ok(None),
                }
            }
        }
    }

    pub(crate) fn resolved_protocol_versions(
        &self,
    ) -> Result<Option<Vec<ProtocolVersion>>, ProxyError> {
        Ok(self.resolved_tls_versions()?.map(|list| {
            list.into_iter()
                .map(|v| match v {
                    TlsVersion::Tls12 => ProtocolVersion::TLSv1_2,
                    TlsVersion::Tls13 => ProtocolVersion::TLSv1_3,
                })
                .collect()
        }))
    }

    fn resolved_cipher_suites(
        &self,
        enabled_versions: Option<&[TlsVersion]>,
    ) -> Result<Vec<rustls::SupportedCipherSuite>, ProxyError> {
        if self.custom_cipher_suites.is_empty() {
            return Ok(Vec::new());
        }
        let mut suites = Vec::with_capacity(self.custom_cipher_suites.len());
        let mut suite_protocols = Vec::with_capacity(self.custom_cipher_suites.len());
        for raw_name in &self.custom_cipher_suites {
            let trimmed = raw_name.trim();
            let Some((proto, suite)) = resolve_ring_cipher_suite(trimmed) else {
                return Err(ProxyError::Tls(format!("不支持的 TLS 密码套件：{trimmed}")));
            };
            if !suites.contains(&suite) {
                suites.push(suite);
                suite_protocols.push(proto);
            }
        }
        if self.protocol_mode == UpstreamTlsProtocolMode::Custom {
            let active = enabled_versions.unwrap_or(&[TlsVersion::Tls13, TlsVersion::Tls12]);
            let mut compatible = Vec::with_capacity(suites.len());
            for (suite, proto) in suites.into_iter().zip(suite_protocols) {
                if active.contains(&proto) {
                    compatible.push(suite);
                }
            }
            if compatible.is_empty() {
                return Err(ProxyError::Tls(
                    "自定义密码套件与启用的 TLS 协议版本不兼容".into(),
                ));
            }
            return Ok(compatible);
        }
        Ok(suites)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct CertCacheKey {
    identity: String,
    enable_http2: bool,
    protocol_filter: ListenerTlsProtocols,
}

struct CachedCertificate {
    key: CertCacheKey,
    issued: Instant,
    config: Arc<ServerConfig>,
}

/// A private issuing identity. Exporting its public certificate does not trust it.
#[derive(Clone)]
pub struct MitmAuthority {
    issuer: Arc<CertificateAuthorityData>,
    cache: Arc<Mutex<VecDeque<CachedCertificate>>>,
}

impl std::fmt::Debug for MitmAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MitmAuthority").finish_non_exhaustive()
    }
}

impl MitmAuthority {
    /// Performs key generation; call from a blocking task, never the UI thread.
    pub fn generate() -> Result<Self, ProxyError> {
        let mut validity = SelfSignedCaConfig::default().validity;
        validity.not_before_skew = CA_CERT_NOT_BEFORE_SKEW;
        let config = SelfSignedCaConfig {
            subject: CertificateSubject {
                organisation_name: Some("MIO local interception".into()),
                common_name: Some("MIO local CA".into()),
            },
            validity,
            ..SelfSignedCaConfig::default()
        };
        let issuer = CertificateAuthorityData::generate(config).map_err(ProxyError::tls)?;
        Ok(Self {
            issuer: Arc::new(issuer),
            cache: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    pub fn from_der(certificate: Vec<u8>, private_key: Vec<u8>) -> Result<Self, ProxyError> {
        Self::from_der_chain(vec![certificate], private_key)
    }

    pub fn from_der_chain(
        certificates: Vec<Vec<u8>>,
        private_key: Vec<u8>,
    ) -> Result<Self, ProxyError> {
        if certificates.is_empty() {
            return Err(ProxyError::Tls("CA 证书链不能为空".into()));
        }
        if certificates.len() > MAX_CUSTOM_CERT_CHAIN_LEN {
            return Err(ProxyError::Tls(format!(
                "CA 证书链超过 {MAX_CUSTOM_CERT_CHAIN_LEN} 张上限"
            )));
        }
        let certs: Vec<CertificateDer<'static>> =
            certificates.into_iter().map(CertificateDer::from).collect();
        let private_key = PrivateKeyDer::try_from(private_key).map_err(ProxyError::tls)?;
        let issuer =
            CertificateAuthorityData::try_new(certs, private_key).map_err(ProxyError::tls)?;
        Ok(Self {
            issuer: Arc::new(issuer),
            cache: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    /// Imports an issuing CA certificate (or issuer-first chain) and private key from PEM strings.
    /// If `key_pem` is blank, the private key is searched inside `cert_pem`.
    pub fn from_pem_pair(cert_pem: &str, key_pem: &str) -> Result<Self, ProxyError> {
        let (certs, key) = parse_server_pem_pair(cert_pem, key_pem)?;
        let issuer = CertificateAuthorityData::try_new(certs, key).map_err(ProxyError::tls)?;
        Ok(Self {
            issuer: Arc::new(issuer),
            cache: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    pub fn certificate_der(&self) -> Vec<u8> {
        self.issuer.certificate_chain()[0].as_ref().to_vec()
    }

    pub fn certificate_chain_der(&self) -> Vec<Vec<u8>> {
        self.issuer
            .certificate_chain()
            .iter()
            .map(|cert| cert.as_ref().to_vec())
            .collect()
    }

    /// Issue a per-host leaf certificate chain under this CA and cryptographically
    /// verify every signature link from the leaf up to the top embedded CA cert.
    pub fn issue_leaf_chain_der(&self, host: &str) -> Result<Vec<Vec<u8>>, ProxyError> {
        let normalized = crate::route::normalize_host(host);
        let (chain, _) = issue_ca_signed_leaf(&self.issuer, &normalized)?;
        for pair in chain.windows(2) {
            let (_, child) = x509_parser::parse_x509_certificate(pair[0].as_ref())
                .map_err(|error| ProxyError::Tls(format!("签发证书解析失败：{error}")))?;
            let (_, parent) = x509_parser::parse_x509_certificate(pair[1].as_ref())
                .map_err(|error| ProxyError::Tls(format!("上级 CA 证书解析失败：{error}")))?;
            if child.issuer() != parent.subject() {
                return Err(ProxyError::Tls("签发证书链 issuer/subject 不匹配".into()));
            }
            child
                .verify_signature(Some(parent.public_key()))
                .map_err(|error| ProxyError::Tls(format!("签发证书链签名校验失败：{error}")))?;
        }
        Ok(chain.into_iter().map(|cert| cert.as_ref().to_vec()).collect())
    }

    pub fn certificate_pem(&self) -> String {
        der_to_pem(&self.certificate_der(), "CERTIFICATE")
    }

    /// Sensitive: the caller must keep this in a private, mode-0600 local file.
    pub fn private_key_der(&self) -> Vec<u8> {
        self.issuer.private_key().secret_der().to_vec()
    }

    /// Sensitive: PEM representation of the CA private key.
    pub fn private_key_pem(&self) -> String {
        let label = match self.issuer.private_key() {
            PrivateKeyDer::Pkcs1(_) => "RSA PRIVATE KEY",
            PrivateKeyDer::Sec1(_) => "EC PRIVATE KEY",
            PrivateKeyDer::Pkcs8(_) => "PRIVATE KEY",
            _ => "PRIVATE KEY",
        };
        der_to_pem(self.issuer.private_key().secret_der(), label)
    }

    pub async fn clear_cache(&self) {
        self.cache.lock().await.clear();
    }

    pub fn spki_sha256_base64(&self) -> Result<String, ProxyError> {
        let cert_der = self.certificate_der();
        let (remaining, parsed) = x509_parser::parse_x509_certificate(&cert_der)
            .map_err(|error| ProxyError::Tls(format!("CA 证书 SPKI 无法解析：{error}")))?;
        if !remaining.is_empty() {
            return Err(ProxyError::Tls("CA 证书包含额外尾随字节".into()));
        }
        let digest = Sha256::digest(parsed.public_key().raw);
        Ok(STANDARD.encode(digest))
    }

    pub(crate) async fn connect_server_config(
        &self,
        host: &str,
        options: &ListenerTlsOptions,
        observed_sni: Arc<std::sync::Mutex<Option<String>>>,
    ) -> Result<TlsServerConfig, ProxyError> {
        let name = crate::route::normalize_host(host);
        if name.parse::<std::net::IpAddr>().is_ok() {
            return Ok(TlsServerConfig::new().with_dynamic_config(Arc::new(
                InvisibleSniCertificate {
                    ca: self.clone(),
                    options: options.clone(),
                    fallback_host: name,
                    observed_sni,
                },
            )));
        }
        let config = self.resolve_rustls_server_config(&name, options).await?;
        let enforce_sni_match = matches!(options.cert_mode, ListenerCertMode::PerHostCaSigned);
        Ok(TlsServerConfig::new().with_dynamic_config(Arc::new(ConnectCertificate {
            name,
            enforce_sni_match,
            config,
            observed_sni,
        })))
    }

    pub(crate) fn invisible_server_config(
        &self,
        options: ListenerTlsOptions,
        fallback_host: String,
        observed_sni: Arc<std::sync::Mutex<Option<String>>>,
    ) -> TlsServerConfig {
        TlsServerConfig::new().with_dynamic_config(Arc::new(InvisibleSniCertificate {
            ca: self.clone(),
            options,
            fallback_host: crate::route::normalize_host(&fallback_host),
            observed_sni,
        }))
    }

    pub(crate) async fn resolve_rustls_server_config(
        &self,
        host: &str,
        options: &ListenerTlsOptions,
    ) -> Result<Arc<ServerConfig>, ProxyError> {
        let name = crate::route::normalize_host(host);
        let cache_identity = match &options.cert_mode {
            ListenerCertMode::PerHostCaSigned => format!("ca:{name}"),
            ListenerCertMode::SelfSigned => {
                let effective = if name.is_empty() { "localhost" } else { &name };
                format!("self:{effective}")
            }
            ListenerCertMode::SpecificHostname(specific) => {
                let normalized = crate::route::normalize_host(specific);
                if normalized.is_empty() {
                    return Err(ProxyError::Tls("固定证书主机名不能为空".into()));
                }
                format!("fixed:{normalized}")
            }
            ListenerCertMode::CustomPem { cert_pem, key_pem } => {
                let mut hasher = Sha256::new();
                hasher.update(cert_pem.as_bytes());
                hasher.update(b"\0");
                hasher.update(key_pem.as_bytes());
                let digest = hasher.finalize();
                format!("pem:{}", STANDARD.encode(digest))
            }
        };
        let cache_key = CertCacheKey {
            identity: cache_identity,
            enable_http2: options.enable_http2,
            protocol_filter: options.protocol_filter,
        };
        {
            let now = Instant::now();
            let mut cache = self.cache.lock().await;
            cache.retain(|entry| now.saturating_duration_since(entry.issued) < CERTIFICATE_CACHE_LIFETIME);
            if let Some(index) = cache.iter().position(|entry| entry.key == cache_key) {
                let entry = cache.remove(index).expect("position came from this cache");
                let config = entry.config.clone();
                cache.push_back(entry);
                return Ok(config);
            }
        }
        let issuer = self.issuer.clone();
        let cert_mode = options.cert_mode.clone();
        let enable_http2 = options.enable_http2;
        let protocol_filter = options.protocol_filter;
        let name_for_signing = name.clone();
        let config = tokio::task::spawn_blocking(move || {
            let (chain, key) = match cert_mode {
                ListenerCertMode::PerHostCaSigned => {
                    issue_ca_signed_leaf(&issuer, &name_for_signing)?
                }
                ListenerCertMode::SelfSigned => {
                    let effective = if name_for_signing.is_empty() {
                        "localhost"
                    } else {
                        &name_for_signing
                    };
                    let parsed_host: Host = effective.parse().map_err(ProxyError::tls)?;
                    let identity =
                        CertificateIdentity::try_from_host(&parsed_host).map_err(ProxyError::tls)?;
                    generate_server_auth(GeneratedServerAuthConfig::SelfSignedLeaf(
                        leaf_cert_request(identity),
                    ))
                    .map_err(ProxyError::tls)?
                }
                ListenerCertMode::SpecificHostname(specific) => {
                    let normalized = crate::route::normalize_host(&specific);
                    issue_ca_signed_leaf(&issuer, &normalized)?
                }
                ListenerCertMode::CustomPem { cert_pem, key_pem } => {
                    parse_server_pem_pair(&cert_pem, &key_pem)?
                }
            };
            build_rustls_server_config(chain, key, enable_http2, protocol_filter)
        })
        .await
        .map_err(|error| ProxyError::Tls(error.to_string()))??;
        let now = Instant::now();
        let mut cache = self.cache.lock().await;
        cache.retain(|entry| now.saturating_duration_since(entry.issued) < CERTIFICATE_CACHE_LIFETIME);
        if let Some(index) = cache.iter().position(|entry| entry.key == cache_key) {
            let entry = cache.remove(index).expect("position came from this cache");
            let existing = entry.config.clone();
            cache.push_back(entry);
            return Ok(existing);
        }
        while cache.len() >= CERTIFICATE_CACHE_LIMIT {
            cache.pop_front();
        }
        cache.push_back(CachedCertificate {
            key: cache_key,
            issued: now,
            config: config.clone(),
        });
        Ok(config)
    }
}

pub(crate) fn append_pem_block_b64(out: &mut String, label: &str, b64: &str) {
    out.push_str("-----BEGIN ");
    out.push_str(label);
    out.push_str("-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        if let Ok(line) = std::str::from_utf8(chunk) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----\n");
}

pub(crate) fn der_to_pem(der: &[u8], label: &str) -> String {
    let encoded = STANDARD.encode(der);
    let mut pem = String::with_capacity(encoded.len() + label.len() * 2 + 48);
    append_pem_block_b64(&mut pem, label, &encoded);
    pem
}

pub(crate) fn parse_pem_cert_chain_and_key(
    cert_pem: &[u8],
    key_pem: &[u8],
    max_chain_len: usize,
    subject_label: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ProxyError> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ProxyError::Tls(format!("{subject_label} PEM 证书解析失败：{err}")))?;
    if certs.is_empty() {
        return Err(ProxyError::Tls(format!("未找到 {subject_label} PEM 证书")));
    }
    if certs.len() > max_chain_len {
        return Err(ProxyError::Tls(format!(
            "{subject_label}证书链超过 {max_chain_len} 张上限"
        )));
    }
    let mut keys: Vec<PrivateKeyDer<'static>> = PrivateKeyDer::pem_slice_iter(key_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ProxyError::Tls(format!("{subject_label} PEM 私钥解析失败：{err}")))?;
    if keys.is_empty() {
        return Err(ProxyError::Tls(format!(
            "未找到受支持的未加密 {subject_label} PEM 私钥"
        )));
    }
    if keys.len() > 1 {
        return Err(ProxyError::Tls(format!(
            "{subject_label}证书材料包含多把私钥，请仅提供一把私钥"
        )));
    }
    Ok((certs, keys.pop().expect("checked single key")))
}

fn parse_server_pem_pair(
    cert_pem: &str,
    key_pem: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ProxyError> {
    let cert_bytes = cert_pem.as_bytes();
    let key_bytes = if key_pem.trim().is_empty() {
        cert_bytes
    } else {
        key_pem.as_bytes()
    };
    parse_pem_cert_chain_and_key(cert_bytes, key_bytes, MAX_CUSTOM_CERT_CHAIN_LEN, "服务端")
}

fn leaf_cert_request(identity: CertificateIdentity) -> LeafCertRequest {
    let mut request = LeafCertRequest::new(identity);
    request.config.validity =
        CertificateValidity::new(LEAF_CERT_LIFETIME, LEAF_CERT_NOT_BEFORE_SKEW);
    request
}

fn issue_ca_signed_leaf(
    issuer: &CertificateAuthorityData,
    normalized_host: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ProxyError> {
    let host: Host = normalized_host.parse().map_err(ProxyError::tls)?;
    let identity = CertificateIdentity::try_from_host(&host).map_err(ProxyError::tls)?;
    let (mut chain, key) = issuer
        .issue_leaf(leaf_cert_request(identity))
        .map_err(ProxyError::tls)?;
    for ca_cert in issuer.certificate_chain() {
        if !chain
            .iter()
            .any(|existing| existing.as_ref() == ca_cert.as_ref())
        {
            chain.push(ca_cert.clone());
        }
    }
    Ok((chain, key))
}

fn build_rustls_server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    enable_http2: bool,
    protocol_filter: ListenerTlsProtocols,
) -> Result<Arc<ServerConfig>, ProxyError> {
    let provider = Arc::clone(shared_crypto_provider());
    let builder = ServerConfig::builder_with_provider(provider);
    let builder = match protocol_filter {
        ListenerTlsProtocols::Default => builder.with_safe_default_protocol_versions(),
        ListenerTlsProtocols::Tls13Only => builder.with_protocol_versions(&[&rustls::version::TLS13]),
        ListenerTlsProtocols::Tls12Only => builder.with_protocol_versions(&[&rustls::version::TLS12]),
    }
    .map_err(ProxyError::tls)?;
    let mut config = builder
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(ProxyError::tls)?;
    config.alpn_protocols = if enable_http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    Ok(Arc::new(config))
}

struct ConnectCertificate {
    name: String,
    enforce_sni_match: bool,
    config: Arc<ServerConfig>,
    observed_sni: Arc<std::sync::Mutex<Option<String>>>,
}

impl DynamicConfigProvider for ConnectCertificate {
    async fn get_config(
        &self,
        hello: rustls::server::ClientHello<'_>,
    ) -> Result<Arc<ServerConfig>, rama::error::BoxError> {
        if let Some(sni) = hello.server_name() {
            let normalized_sni = crate::route::normalize_host(sni);
            *crate::lock_unpoison(&self.observed_sni) = Some(normalized_sni.clone());
            if self.enforce_sni_match && normalized_sni != self.name {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "TLS SNI does not match CONNECT authority",
                )
                .into());
            }
        }
        Ok(self.config.clone())
    }
}

struct InvisibleSniCertificate {
    ca: MitmAuthority,
    options: ListenerTlsOptions,
    fallback_host: String,
    observed_sni: Arc<std::sync::Mutex<Option<String>>>,
}

impl DynamicConfigProvider for InvisibleSniCertificate {
    async fn get_config(
        &self,
        hello: rustls::server::ClientHello<'_>,
    ) -> Result<Arc<ServerConfig>, rama::error::BoxError> {
        let sni = hello
            .server_name()
            .map(crate::route::normalize_host)
            .filter(|name| !name.is_empty());
        if let Some(ref host) = sni {
            *crate::lock_unpoison(&self.observed_sni) = Some(host.clone());
        }
        let target_host = sni.as_deref().unwrap_or_else(|| {
            if self.fallback_host.is_empty() {
                "localhost"
            } else {
                &self.fallback_host
            }
        });
        self.ca
            .resolve_rustls_server_config(target_host, &self.options)
            .await
            .map_err(Into::into)
    }
}

#[derive(Clone, Debug, Default)]
pub enum UpstreamTls {
    #[default]
    System,
    Insecure,
    CustomRoots {
        certificates_der: Vec<Vec<u8>>,
        include_system: bool,
    },
}

pub type ServerCertObservedCallback = Arc<dyn Fn(ObservedCertInfo) + Send + Sync>;

#[derive(Clone)]
pub struct UpstreamTlsConfig {
    pub trust: UpstreamTls,
    pub options: UpstreamTlsOptions,
    pub client_certificates: Arc<[(crate::ClientCertificateRule, crate::ParsedClientIdentity)]>,
    pub on_server_cert_observed: Option<ServerCertObservedCallback>,
}

impl Default for UpstreamTlsConfig {
    fn default() -> Self {
        Self {
            trust: UpstreamTls::System,
            options: UpstreamTlsOptions::default(),
            client_certificates: Arc::from([]),
            on_server_cert_observed: None,
        }
    }
}

impl std::fmt::Debug for UpstreamTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamTlsConfig")
            .field("trust", &self.trust)
            .field("options", &self.options)
            .field("client_certificates_len", &self.client_certificates.len())
            .field(
                "on_server_cert_observed_configured",
                &self.on_server_cert_observed.is_some(),
            )
            .finish()
    }
}

impl UpstreamTls {
    pub fn certificate_display_name(certificate: &[u8]) -> Result<String, ProxyError> {
        Self::CustomRoots {
            certificates_der: vec![certificate.to_vec()],
            include_system: false,
        }
        .validate()?;
        let (remaining, parsed) = x509_parser::parse_x509_certificate(certificate)
            .map_err(|error| ProxyError::Tls(format!("证书名称无法解析：{error}")))?;
        if !remaining.is_empty() {
            return Err(ProxyError::Tls("证书包含额外字节，未忽略".into()));
        }
        if let Some(common_name) = parsed.subject().iter_common_name().next() {
            return common_name
                .as_str()
                .map(str::to_owned)
                .map_err(|error| ProxyError::Tls(format!("证书 Common Name 无法显示：{error}")));
        }
        Ok(parsed.subject().to_string())
    }

    pub fn validate(&self) -> Result<(), ProxyError> {
        let Self::CustomRoots {
            certificates_der, ..
        } = self
        else {
            return Ok(());
        };
        if certificates_der.is_empty() || certificates_der.len() > 32 {
            return Err(ProxyError::Tls(
                "自定义信任根列表须包含 1 到 32 个证书".into(),
            ));
        }
        let mut roots = rustls::RootCertStore::empty();
        for (index, certificate) in certificates_der.iter().enumerate() {
            if certificate.is_empty() || certificate.len() > 64 * 1024 {
                return Err(ProxyError::Tls(
                    "自定义根证书须为非空且不超过 64 KiB 的 DER 编码".into(),
                ));
            }
            roots
                .add(CertificateDer::from(certificate.as_slice()))
                .map_err(|error| {
                    ProxyError::Tls(format!("第 {} 个自定义根证书 DER 无效：{error}", index + 1))
                })?;
        }
        Ok(())
    }

    pub fn validate_with_options(&self, options: &UpstreamTlsOptions) -> Result<(), ProxyError> {
        self.validate()?;
        options.validate()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn config(
        &self,
        version: Option<mio_plugin_api::HttpVersion>,
    ) -> Result<TlsClientConfig, ProxyError> {
        self.config_with_options(version, &UpstreamTlsOptions::default())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ClientAlpnMode {
    Auto,
    Http1,
    Http2,
}

impl ClientAlpnMode {
    pub(crate) fn from_http_version(version: Option<mio_plugin_api::HttpVersion>) -> Self {
        match version {
            Some(mio_plugin_api::HttpVersion::Http2) => Self::Http2,
            Some(_) => Self::Http1,
            None => Self::Auto,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TrustModeKey {
    System,
    Insecure,
    CustomRoots {
        include_system: bool,
        certs_sha256: [u8; 32],
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ClientTlsCacheKey {
    pub(crate) trust: TrustModeKey,
    pub(crate) alpn: ClientAlpnMode,
    pub(crate) options: UpstreamTlsOptions,
    pub(crate) client_cert_sha256: Option<[u8; 32]>,
    pub(crate) store_server_cert_chain: bool,
}

struct CachedClientTls {
    key: ClientTlsCacheKey,
    tls_config: TlsClientConfig,
}

fn hash_cert_ders(certificates_der: &[Vec<u8>]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((certificates_der.len() as u64).to_le_bytes());
    for cert in certificates_der {
        hasher.update((cert.len() as u64).to_le_bytes());
        hasher.update(cert.as_slice());
    }
    hasher.finalize().into()
}

fn hash_client_identity(identity: &crate::ParsedClientIdentity) -> [u8; 32] {
    let mut hasher = Sha256::new();
    let chain = identity.certificate_chain();
    hasher.update((chain.len() as u64).to_le_bytes());
    for cert in chain {
        let bytes = cert.as_ref();
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    let key = identity.private_key();
    let key_bytes = key.secret_der();
    hasher.update((key_bytes.len() as u64).to_le_bytes());
    hasher.update(key_bytes);
    hasher.finalize().into()
}

#[derive(Debug)]
struct InsecureCustomCertVerifier(Vec<rustls::SignatureScheme>);

impl rustls::client::danger::ServerCertVerifier for InsecureCustomCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.clone()
    }
}

fn build_custom_root_store(
    trust: &UpstreamTls,
) -> Result<rustls::RootCertStore, ProxyError> {
    let mut roots = rustls::RootCertStore::empty();
    let include_system = match trust {
        UpstreamTls::System => true,
        UpstreamTls::Insecure => false,
        UpstreamTls::CustomRoots {
            certificates_der,
            include_system,
        } => {
            for der in certificates_der {
                roots
                    .add(CertificateDer::from(der.clone()))
                    .map_err(ProxyError::tls)?;
            }
            *include_system
        }
    };
    if include_system {
        let anchors = rama::crypto::native_certs::shared_native_trust_anchors();
        let (_, _) = roots.add_parsable_certificates(anchors.iter().cloned());
    }
    Ok(roots)
}

fn build_tls_client_config(
    trust: &UpstreamTls,
    alpn: ClientAlpnMode,
    options: &UpstreamTlsOptions,
    client_identity: Option<&crate::ParsedClientIdentity>,
    store_server_cert_chain: bool,
) -> Result<TlsClientConfig, ProxyError> {
    trust.validate()?;
    let tls_versions = options.resolved_tls_versions()?;
    let supported_versions = options.resolved_protocol_versions()?;
    let custom_suites = options.resolved_cipher_suites(tls_versions.as_deref())?;
    let mut config = match alpn {
        ClientAlpnMode::Http2 => TlsClientConfig::new().with_alpn_http_2(),
        ClientAlpnMode::Http1 => TlsClientConfig::new().with_alpn_http_1(),
        ClientAlpnMode::Auto => TlsClientConfig::new().with_alpn_http_auto(),
    }
    .with_keylog(KeyLogIntent::Disabled)
    .with_store_server_cert_chain(store_server_cert_chain);

    if let Some(versions) = supported_versions {
        config = config.with_supported_versions(versions);
    }

    config = match trust {
        UpstreamTls::System => config.with_server_verify(ServerVerifyMode::Auto),
        UpstreamTls::Insecure => config.with_server_verify(ServerVerifyMode::Disable),
        UpstreamTls::CustomRoots {
            certificates_der,
            include_system,
        } => {
            let anchors: Vec<CertificateDer<'static>> = certificates_der
                .iter()
                .map(|cert| CertificateDer::from(cert.clone()))
                .collect();
            let config = config.with_server_verify(ServerVerifyMode::Auto);
            if *include_system {
                config
                    .try_with_extra_server_trust_anchors(anchors)
                    .map_err(ProxyError::tls)?
            } else {
                config
                    .try_with_server_trust_anchors(anchors)
                    .map_err(ProxyError::tls)?
            }
        }
    };

    if let Some(identity) = client_identity {
        config = identity.apply_to_tls_config(config);
    }

    if options.protocol_mode == UpstreamTlsProtocolMode::Custom && !custom_suites.is_empty() {
        let mut provider = rustls::crypto::ring::default_provider();
        provider.cipher_suites = custom_suites;
        let provider = Arc::new(provider);
        let rustls_versions: Vec<&'static rustls::SupportedProtocolVersion> = tls_versions
            .unwrap_or_else(|| vec![TlsVersion::Tls13, TlsVersion::Tls12])
            .into_iter()
            .map(|v| match v {
                TlsVersion::Tls13 => &rustls::version::TLS13,
                TlsVersion::Tls12 => &rustls::version::TLS12,
            })
            .collect();
        let verifier: Arc<dyn rustls::client::danger::ServerCertVerifier> = match trust {
            UpstreamTls::Insecure => Arc::new(InsecureCustomCertVerifier(
                provider.signature_verification_algorithms.supported_schemes(),
            )),
            UpstreamTls::System | UpstreamTls::CustomRoots { .. } => {
                let roots = build_custom_root_store(trust)?;
                rustls::client::WebPkiServerVerifier::builder_with_provider(
                    Arc::new(roots),
                    provider.clone(),
                )
                .build()
                .map_err(ProxyError::tls)?
            }
        };
        let disable_resumption = options.disable_session_resume;
        config = config.with_modify_rustls_config(move |incoming| {
            let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
                .with_protocol_versions(&rustls_versions)
                .map_err(rama::error::BoxError::from)?;
            let mut next = builder
                .dangerous()
                .with_custom_certificate_verifier(verifier.clone())
                .with_client_cert_resolver(incoming.client_auth_cert_resolver);
            next.alpn_protocols = incoming.alpn_protocols;
            next.key_log = incoming.key_log;
            if disable_resumption {
                next.resumption = rustls::client::Resumption::disabled();
            } else {
                next.resumption = incoming.resumption;
            }
            Ok(next)
        });
    } else if options.disable_session_resume {
        config = config.with_modify_rustls_config(|mut cfg| {
            cfg.resumption = rustls::client::Resumption::disabled();
            Ok(cfg)
        });
    }

    Ok(config)
}

fn client_tls_cache() -> &'static std::sync::Mutex<VecDeque<CachedClientTls>> {
    static CACHE: OnceLock<std::sync::Mutex<VecDeque<CachedClientTls>>> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(VecDeque::with_capacity(CLIENT_TLS_CACHE_LIMIT)))
}

impl UpstreamTls {
    pub(crate) fn config_with_options(
        &self,
        version: Option<mio_plugin_api::HttpVersion>,
        options: &UpstreamTlsOptions,
    ) -> Result<TlsClientConfig, ProxyError> {
        self.config_and_key(version, options, None, false)
            .map(|(config, _)| config)
    }

    pub(crate) fn config_and_key(
        &self,
        version: Option<mio_plugin_api::HttpVersion>,
        options: &UpstreamTlsOptions,
        client_identity: Option<&crate::ParsedClientIdentity>,
        store_server_cert_chain: bool,
    ) -> Result<(TlsClientConfig, ClientTlsCacheKey), ProxyError> {
        options.validate()?;
        let alpn = ClientAlpnMode::from_http_version(version);
        let trust_key = match self {
            Self::System => TrustModeKey::System,
            Self::Insecure => TrustModeKey::Insecure,
            Self::CustomRoots {
                certificates_der,
                include_system,
            } => {
                self.validate()?;
                TrustModeKey::CustomRoots {
                    include_system: *include_system,
                    certs_sha256: hash_cert_ders(certificates_der),
                }
            }
        };
        let key = ClientTlsCacheKey {
            trust: trust_key,
            alpn,
            options: options.clone(),
            client_cert_sha256: client_identity.map(hash_client_identity),
            store_server_cert_chain,
        };
        {
            let mut cache = crate::lock_unpoison(client_tls_cache());
            if let Some(index) = cache.iter().position(|entry| entry.key == key) {
                let entry = cache.remove(index).expect("position came from this cache");
                let config = entry.tls_config.clone();
                cache.push_back(entry);
                return Ok((config, key));
            }
        }
        let tls_config = build_tls_client_config(
            self,
            alpn,
            options,
            client_identity,
            store_server_cert_chain,
        )?;
        {
            let mut cache = crate::lock_unpoison(client_tls_cache());
            if let Some(index) = cache.iter().position(|entry| entry.key == key) {
                let entry = cache.remove(index).expect("position came from this cache");
                let existing = entry.tls_config.clone();
                cache.push_back(entry);
                return Ok((existing, key));
            }
            while cache.len() >= CLIENT_TLS_CACHE_LIMIT {
                cache.pop_front();
            }
            cache.push_back(CachedClientTls {
                key: key.clone(),
                tls_config: tls_config.clone(),
            });
        }
        Ok((tls_config, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rama::tls::rustls::client::RustlsTlsConnectorConfig;

    #[test]
    fn certificate_name_comes_from_validated_subject() {
        let certificate = MitmAuthority::generate().unwrap().certificate_der();
        assert_eq!(
            UpstreamTls::certificate_display_name(&certificate).unwrap(),
            "MIO local CA"
        );
        let authority = MitmAuthority::generate().unwrap();
        let spki = authority.spki_sha256_base64().unwrap();
        assert_eq!(spki.len(), 44);
        assert!(UpstreamTls::certificate_display_name(&[0x30, 0]).is_err());
        let mut trailing = certificate;
        trailing.push(0);
        assert!(UpstreamTls::certificate_display_name(&trailing).is_err());
    }

    #[test]
    fn ca_generate_and_pem_pair_import_roundtrip() {
        let first = MitmAuthority::generate().unwrap();
        let second = MitmAuthority::generate().unwrap();
        assert_ne!(first.certificate_der(), second.certificate_der());

        let cert_pem = first.certificate_pem();
        let key_pem = first.private_key_pem();
        let imported = MitmAuthority::from_pem_pair(&cert_pem, &key_pem).unwrap();
        assert_eq!(imported.certificate_der(), first.certificate_der());
        assert_eq!(imported.private_key_der(), first.private_key_der());

        let combined = format!("{cert_pem}\n{key_pem}");
        let imported_combined = MitmAuthority::from_pem_pair(&combined, "").unwrap();
        assert_eq!(imported_combined.certificate_der(), first.certificate_der());

        assert!(MitmAuthority::from_pem_pair(&cert_pem, &second.private_key_pem()).is_err());
        assert!(MitmAuthority::from_pem_pair("not a pem", &key_pem).is_err());
    }

    #[test]
    fn ca_from_der_chain_preserves_signed_intermediate_chain() {
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

        let combined_pem = format!("{}{}", inter_cert.pem(), root_cert.pem());
        let from_pem =
            MitmAuthority::from_pem_pair(&combined_pem, &inter_key.serialize_pem()).unwrap();
        let chain = from_pem.certificate_chain_der();
        assert_eq!(
            chain,
            vec![
                inter_cert.der().as_ref().to_vec(),
                root_cert.der().as_ref().to_vec(),
            ]
        );
        let imported = MitmAuthority::from_der_chain(chain.clone(), from_pem.private_key_der())
            .unwrap();
        assert_eq!(imported.certificate_chain_der(), chain);
        let (leaf_chain, _) =
            issue_ca_signed_leaf(&imported.issuer, "lab.example.invalid").unwrap();
        assert_eq!(leaf_chain.len(), 3);
        assert_eq!(leaf_chain[1].as_ref(), inter_cert.der().as_ref());
        assert_eq!(leaf_chain[2].as_ref(), root_cert.der().as_ref());
    }

    #[tokio::test]
    async fn listener_cert_modes_alpn_and_protocol_filters_build_valid_configs() {
        let ca = MitmAuthority::generate().unwrap();

        let h1_only = ca
            .resolve_rustls_server_config(
                "example.com",
                &ListenerTlsOptions {
                    enable_http2: false,
                    protocol_filter: ListenerTlsProtocols::Tls13Only,
                    cert_mode: ListenerCertMode::PerHostCaSigned,
                },
            )
            .await
            .unwrap();
        assert_eq!(h1_only.alpn_protocols, vec![b"http/1.1".to_vec()]);

        let self_signed = ca
            .resolve_rustls_server_config(
                "self.local",
                &ListenerTlsOptions {
                    enable_http2: true,
                    protocol_filter: ListenerTlsProtocols::Tls12Only,
                    cert_mode: ListenerCertMode::SelfSigned,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            self_signed.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );

        let fixed = ca
            .resolve_rustls_server_config(
                "any.local",
                &ListenerTlsOptions {
                    enable_http2: true,
                    protocol_filter: ListenerTlsProtocols::Default,
                    cert_mode: ListenerCertMode::SpecificHostname("fixed.example.com".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fixed.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );

        let (leaf_chain, leaf_key) = generate_server_auth(
            GeneratedServerAuthConfig::SelfSignedLeaf(LeafCertRequest::new(
                CertificateIdentity::try_from_host(
                    &"custom.example.invalid".parse::<Host>().unwrap(),
                )
                    .unwrap(),
            )),
        )
        .unwrap();
        let custom_mode = ListenerCertMode::CustomPem {
            cert_pem: der_to_pem(leaf_chain[0].as_ref(), "CERTIFICATE"),
            key_pem: der_to_pem(leaf_key.secret_der(), "PRIVATE KEY"),
        };
        custom_mode.validate().unwrap();
        let custom = ca
            .resolve_rustls_server_config(
                "ignored.local",
                &ListenerTlsOptions {
                    enable_http2: false,
                    protocol_filter: ListenerTlsProtocols::Default,
                    cert_mode: custom_mode,
                },
            )
            .await
            .unwrap();
        assert_eq!(custom.alpn_protocols, vec![b"http/1.1".to_vec()]);

        assert!(ListenerCertMode::SpecificHostname("".into()).validate().is_err());
        assert!(
            ListenerCertMode::CustomPem {
                cert_pem: "bad".into(),
                key_pem: "bad".into(),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn parse_observed_cert_extracts_metadata_and_sans() {
        let ca = MitmAuthority::generate().unwrap();
        let (chain, _) = issue_ca_signed_leaf(&ca.issuer, "api.example.com").unwrap();
        let info = parse_observed_cert("[api.example.com].", 8443, chain[0].as_ref()).unwrap();
        assert_eq!(info.host, "api.example.com");
        assert_eq!(info.port, 8443);
        assert!(info.issuer.contains("MIO local CA"));
        assert!(!info.valid_from.is_empty());
        assert!(!info.valid_to.is_empty());
        assert_eq!(info.sans, vec!["api.example.com".to_owned()]);
        assert_eq!(info.sha256_fingerprint.split(':').count(), 32);
        assert!(info.pem.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(info.pem.ends_with("-----END CERTIFICATE-----\n"));
    }

    #[test]
    fn upstream_tls_options_wire_versions_and_disable_session_resume() {
        let opts = UpstreamTlsOptions {
            disable_session_resume: true,
            protocol_mode: UpstreamTlsProtocolMode::Tls13Only,
            tls_min_version: None,
            tls_max_version: None,
            ..Default::default()
        };
        let cfg = UpstreamTls::System.config_with_options(None, &opts).unwrap();
        let rustls_cfg = rustls::ClientConfig::try_from(RustlsTlsConnectorConfig::from_extensions(
            cfg.as_extensions(),
        ))
        .unwrap();
        assert!(rustls_cfg.alpn_protocols.contains(&b"h2".to_vec()));

        let tls12 = UpstreamTlsOptions {
            disable_session_resume: false,
            protocol_mode: UpstreamTlsProtocolMode::Tls12Only,
            tls_min_version: None,
            tls_max_version: None,
            ..Default::default()
        };
        let cfg12 = UpstreamTls::System
            .config_with_options(None, &tls12)
            .unwrap();
        assert!(
            rustls::ClientConfig::try_from(RustlsTlsConnectorConfig::from_extensions(
                cfg12.as_extensions()
            ))
            .is_ok()
        );

        let invalid = UpstreamTlsOptions {
            disable_session_resume: false,
            protocol_mode: UpstreamTlsProtocolMode::Default,
            tls_min_version: Some(TlsVersion::Tls13),
            tls_max_version: Some(TlsVersion::Tls12),
            ..Default::default()
        };
        assert!(UpstreamTls::System.config_with_options(None, &invalid).is_err());

        let empty_custom = UpstreamTlsOptions {
            disable_session_resume: false,
            protocol_mode: UpstreamTlsProtocolMode::Custom,
            tls_min_version: None,
            tls_max_version: None,
            custom_protocols: Vec::new(),
            custom_cipher_suites: Vec::new(),
        };
        assert!(
            UpstreamTls::System
                .config_with_options(None, &empty_custom)
                .is_err()
        );

        let incompatible_custom = UpstreamTlsOptions {
            disable_session_resume: false,
            protocol_mode: UpstreamTlsProtocolMode::Custom,
            tls_min_version: None,
            tls_max_version: None,
            custom_protocols: vec![TlsVersion::Tls12],
            custom_cipher_suites: vec!["TLS13_CHACHA20_POLY1305_SHA256".into()],
        };
        assert!(
            UpstreamTls::System
                .config_with_options(None, &incompatible_custom)
                .is_err()
        );

        let unknown_suite = UpstreamTlsOptions {
            disable_session_resume: false,
            protocol_mode: UpstreamTlsProtocolMode::Custom,
            tls_min_version: None,
            tls_max_version: None,
            custom_protocols: vec![TlsVersion::Tls13],
            custom_cipher_suites: vec!["NO_SUCH_CIPHER".into()],
        };
        assert!(
            UpstreamTls::System
                .config_with_options(None, &unknown_suite)
                .is_err()
        );

        let mixed_custom = UpstreamTlsOptions {
            disable_session_resume: false,
            protocol_mode: UpstreamTlsProtocolMode::Custom,
            tls_min_version: None,
            tls_max_version: None,
            custom_protocols: vec![TlsVersion::Tls13],
            custom_cipher_suites: vec![
                "TLS13_CHACHA20_POLY1305_SHA256".into(),
                "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256".into(),
            ],
        };
        assert_eq!(
            mixed_custom
                .resolved_cipher_suites(mixed_custom.resolved_tls_versions().unwrap().as_deref())
                .unwrap()
                .len(),
            1
        );
        assert!(UpstreamTls::System.config_with_options(None, &mixed_custom).is_ok());

        let ca_der = MitmAuthority::generate().unwrap().certificate_der();
        for (trust, alpn_ver, store_chain) in [
            (UpstreamTls::System, Some(mio_plugin_api::HttpVersion::Http11), false),
            (UpstreamTls::Insecure, Some(mio_plugin_api::HttpVersion::Http2), true),
            (
                UpstreamTls::CustomRoots {
                    certificates_der: vec![ca_der.clone()],
                    include_system: false,
                },
                None,
                true,
            ),
            (
                UpstreamTls::CustomRoots {
                    certificates_der: vec![ca_der],
                    include_system: true,
                },
                Some(mio_plugin_api::HttpVersion::Http11),
                false,
            ),
        ] {
            let (native_cfg, _) = trust
                .config_and_key(alpn_ver, &opts, None, store_chain)
                .unwrap();
            let resolved = rustls::ClientConfig::try_from(RustlsTlsConnectorConfig::from_extensions(
                native_cfg.as_extensions(),
            ))
            .unwrap();
            assert!(!resolved.alpn_protocols.is_empty());
        }
    }

    #[test]
    fn upstream_tls_config_rejects_malformed_roots_before_connecting() {
        let good_der = MitmAuthority::generate().unwrap().certificate_der();
        for include_system in [false, true] {
            assert!(
                UpstreamTls::CustomRoots {
                    certificates_der: vec![good_der.clone()],
                    include_system,
                }
                .config(None)
                .is_ok()
            );
            for certificates_der in [
                Vec::new(),
                vec![vec![0x30, 0]],
                vec![good_der.clone(), vec![0x30, 0]],
                vec![good_der.clone(); 33],
                vec![vec![0x30; 64 * 1024 + 1]],
            ] {
                assert!(
                    UpstreamTls::CustomRoots {
                        certificates_der,
                        include_system,
                    }
                    .config(None)
                    .is_err(),
                    "a malformed trust configuration must fail before network use"
                );
            }
        }
    }

    async fn spawn_test_tls_server(
        versions: &[&'static rustls::SupportedProtocolVersion],
        suites: Vec<rustls::SupportedCipherSuite>,
        leaf_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
        leaf_key: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> std::net::SocketAddr {
        let mut provider = rustls::crypto::ring::default_provider();
        provider.cipher_suites = suites;
        let server_cfg = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(versions)
            .expect("valid test server versions")
            .with_no_client_auth()
            .with_single_cert(leaf_chain, leaf_key)
            .expect("valid test server cert");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_cfg));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                let _ = acceptor.accept(tcp).await;
            }
        });
        addr
    }

    async fn connect_with_upstream_options(
        addr: std::net::SocketAddr,
        leaf_der: &[u8],
        opts: &UpstreamTlsOptions,
    ) -> Result<(Option<rustls::ProtocolVersion>, Option<rustls::CipherSuite>), String> {
        let native_cfg = UpstreamTls::CustomRoots {
            certificates_der: vec![leaf_der.to_vec()],
            include_system: false,
        }
        .config_with_options(None, opts)
        .map_err(|e| e.to_string())?;
        let rustls_cfg =
            rustls::ClientConfig::try_from(RustlsTlsConnectorConfig::from_extensions(
                native_cfg.as_extensions(),
            ))
            .map_err(|e| e.to_string())?;
        let connector = tokio_rustls::TlsConnector::from(Arc::new(rustls_cfg));
        let tcp = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| e.to_string())?;
        let server_name =
            rustls::pki_types::ServerName::try_from("custom.example.invalid").unwrap();
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| e.to_string())?;
        let (_, conn) = tls.get_ref();
        Ok((
            conn.protocol_version(),
            conn.negotiated_cipher_suite().map(|cs| cs.suite()),
        ))
    }

    #[tokio::test]
    async fn custom_tls_protocols_and_ciphers_enforced_in_loopback_handshakes() {
        use rustls::crypto::ring::cipher_suite;

        let (leaf_chain, leaf_key) = generate_server_auth(
            GeneratedServerAuthConfig::SelfSignedLeaf(LeafCertRequest::new(
                CertificateIdentity::try_from_host(
                    &"custom.example.invalid".parse::<Host>().unwrap(),
                )
                    .unwrap(),
            )),
        )
        .unwrap();
        let leaf_der = leaf_chain[0].as_ref().to_vec();

        // 1. TLS 1.3 custom cipher suite match (server prefers AES-256 first, client allows only Chacha20)
        let addr_tls13 = spawn_test_tls_server(
            &[&rustls::version::TLS13],
            vec![
                cipher_suite::TLS13_AES_256_GCM_SHA384,
                cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
            ],
            leaf_chain.clone(),
            leaf_key.clone_key(),
        )
        .await;
        let tls13_chacha = UpstreamTlsOptions {
            protocol_mode: UpstreamTlsProtocolMode::Custom,
            custom_protocols: vec![TlsVersion::Tls13],
            custom_cipher_suites: vec!["TLS13_CHACHA20_POLY1305_SHA256".into()],
            ..Default::default()
        };
        let (ver, suite) = connect_with_upstream_options(addr_tls13, &leaf_der, &tls13_chacha)
            .await
            .expect("TLS 1.3 matching custom cipher handshake succeeds");
        assert_eq!(ver, Some(rustls::ProtocolVersion::TLSv1_3));
        assert_eq!(
            suite,
            Some(rustls::CipherSuite::TLS13_CHACHA20_POLY1305_SHA256)
        );

        // 2. TLS 1.3 custom cipher suite mismatch fails
        let addr_tls13_aes_only = spawn_test_tls_server(
            &[&rustls::version::TLS13],
            vec![cipher_suite::TLS13_AES_256_GCM_SHA384],
            leaf_chain.clone(),
            leaf_key.clone_key(),
        )
        .await;
        assert!(
            connect_with_upstream_options(addr_tls13_aes_only, &leaf_der, &tls13_chacha)
                .await
                .is_err(),
            "mismatched custom TLS 1.3 cipher suite must fail handshake"
        );

        // 3. TLS 1.2 custom cipher suite match succeeds
        let addr_tls12 = spawn_test_tls_server(
            &[&rustls::version::TLS12],
            vec![
                cipher_suite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
                cipher_suite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
            ],
            leaf_chain.clone(),
            leaf_key.clone_key(),
        )
        .await;
        let tls12_chacha = UpstreamTlsOptions {
            protocol_mode: UpstreamTlsProtocolMode::Custom,
            custom_protocols: vec![TlsVersion::Tls12],
            custom_cipher_suites: vec!["TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256".into()],
            ..Default::default()
        };
        let (ver, suite) = connect_with_upstream_options(addr_tls12, &leaf_der, &tls12_chacha)
            .await
            .expect("TLS 1.2 matching custom cipher handshake succeeds");
        assert_eq!(ver, Some(rustls::ProtocolVersion::TLSv1_2));
        assert_eq!(
            suite,
            Some(rustls::CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256)
        );

        // 4. TLS 1.2 custom protocol-only (no cipher restriction) fails against TLS 1.3-only server
        let addr_tls13_only = spawn_test_tls_server(
            &[&rustls::version::TLS13],
            vec![cipher_suite::TLS13_AES_256_GCM_SHA384],
            leaf_chain,
            leaf_key,
        )
        .await;
        let tls12_any_suite = UpstreamTlsOptions {
            protocol_mode: UpstreamTlsProtocolMode::Custom,
            custom_protocols: vec![TlsVersion::Tls12],
            custom_cipher_suites: Vec::new(),
            ..Default::default()
        };
        assert!(
            connect_with_upstream_options(addr_tls13_only, &leaf_der, &tls12_any_suite)
                .await
                .is_err(),
            "TLS 1.2 custom protocol filter must reject TLS 1.3-only server"
        );
    }

    #[test]
    fn generated_ca_and_leaf_certificates_apply_clock_skew_and_bounded_lifetime() {
        let ca = MitmAuthority::generate().unwrap();
        let ca_der = ca.certificate_der();
        let (_, parsed_ca) = x509_parser::parse_x509_certificate(&ca_der).unwrap();
        let ca_validity = parsed_ca.validity();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // At least 13 days of backdated clock skew before current time.
        assert!(
            now_secs - ca_validity.not_before.timestamp() >= 13 * 24 * 60 * 60,
            "CA notBefore should tolerate client clock skew"
        );

        let (leaf_chain, _) = issue_ca_signed_leaf(&ca.issuer, "lab.example.invalid").unwrap();
        let (_, parsed_leaf) = x509_parser::parse_x509_certificate(leaf_chain[0].as_ref()).unwrap();
        let leaf_validity = parsed_leaf.validity();
        assert!(now_secs - leaf_validity.not_before.timestamp() >= 13 * 24 * 60 * 60);
        let span_secs = leaf_validity.not_after.timestamp() - leaf_validity.not_before.timestamp();
        assert!(
            span_secs <= 195 * 24 * 60 * 60,
            "leaf lifetime must stay well under mobile/browser 397-day limit, got {span_secs}s"
        );
    }

    #[test]
    fn issue_leaf_chain_der_verifies_signed_chain_for_generated_ca() {
        let ca = MitmAuthority::generate().unwrap();
        let chain = ca.issue_leaf_chain_der("lab.example.invalid.").unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[1], ca.certificate_der());
        assert!(ca.issue_leaf_chain_der("").is_err());
    }
}
