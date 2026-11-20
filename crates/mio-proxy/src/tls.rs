use crate::ProxyError;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rama::crypto::{
    cert::{
        CertificateAuthorityData, CertificateIdentity, CertificateSubject,
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
        client::{RustlsClientConfigExt, client_root_certs},
        dep::rustls::{self, ServerConfig},
        server::{DynamicConfigProvider, RustlsServerConfigExt},
        verify::NoServerCertVerifier,
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

pub(crate) fn shared_crypto_provider() -> &'static Arc<rustls::crypto::CryptoProvider> {
    static PROVIDER: OnceLock<Arc<rustls::crypto::CryptoProvider>> = OnceLock::new();
    PROVIDER.get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
}

fn shared_system_root_certs() -> &'static Arc<rustls::RootCertStore> {
    static SYSTEM_ROOTS: OnceLock<Arc<rustls::RootCertStore>> = OnceLock::new();
    SYSTEM_ROOTS.get_or_init(client_root_certs)
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
                let normalized = normalize_cert_host(hostname);
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
        host: normalize_cert_host(host),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UpstreamTlsProtocolMode {
    #[default]
    Default,
    AllSupported,
    Tls12Only,
    Tls13Only,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct UpstreamTlsOptions {
    pub disable_session_resume: bool,
    pub protocol_mode: UpstreamTlsProtocolMode,
    pub tls_min_version: Option<TlsVersion>,
    pub tls_max_version: Option<TlsVersion>,
}

impl UpstreamTlsOptions {
    pub fn validate(&self) -> Result<(), ProxyError> {
        let _ = self.resolved_protocol_versions()?;
        Ok(())
    }

    pub(crate) fn resolved_protocol_versions(
        &self,
    ) -> Result<Option<Vec<ProtocolVersion>>, ProxyError> {
        if let (Some(min), Some(max)) = (self.tls_min_version, self.tls_max_version)
            && min > max
        {
            return Err(ProxyError::Tls("TLS 最低版本不能高于最高版本".into()));
        }
        match self.protocol_mode {
            UpstreamTlsProtocolMode::Tls12Only => Ok(Some(vec![ProtocolVersion::TLSv1_2])),
            UpstreamTlsProtocolMode::Tls13Only => Ok(Some(vec![ProtocolVersion::TLSv1_3])),
            UpstreamTlsProtocolMode::Default | UpstreamTlsProtocolMode::AllSupported => {
                match (self.tls_min_version, self.tls_max_version) {
                    (Some(TlsVersion::Tls13), _) => Ok(Some(vec![ProtocolVersion::TLSv1_3])),
                    (_, Some(TlsVersion::Tls12)) => Ok(Some(vec![ProtocolVersion::TLSv1_2])),
                    (Some(TlsVersion::Tls12), Some(TlsVersion::Tls13))
                    | (Some(TlsVersion::Tls12), None)
                    | (None, Some(TlsVersion::Tls13)) => {
                        Ok(Some(vec![ProtocolVersion::TLSv1_3, ProtocolVersion::TLSv1_2]))
                    }
                    (None, None) => Ok(None),
                }
            }
        }
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
        let config = SelfSignedCaConfig {
            subject: CertificateSubject {
                organisation_name: Some("MIO local interception".into()),
                common_name: Some("MIO local CA".into()),
            },
            ..SelfSignedCaConfig::default()
        };
        let issuer = CertificateAuthorityData::generate(config).map_err(ProxyError::tls)?;
        Ok(Self {
            issuer: Arc::new(issuer),
            cache: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    /// Generates a fresh replacement CA identity with an empty leaf certificate cache.
    pub fn regenerate() -> Result<Self, ProxyError> {
        Self::generate()
    }

    pub fn from_der(certificate: Vec<u8>, private_key: Vec<u8>) -> Result<Self, ProxyError> {
        let private_key = PrivateKeyDer::try_from(private_key).map_err(ProxyError::tls)?;
        let issuer =
            CertificateAuthorityData::try_new(vec![CertificateDer::from(certificate)], private_key)
                .map_err(ProxyError::tls)?;
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

    pub(crate) async fn server_config_with_options(
        &self,
        host: &str,
        options: &ListenerTlsOptions,
    ) -> Result<TlsServerConfig, ProxyError> {
        let name = normalize_cert_host(host);
        let config = self.resolve_rustls_server_config(&name, options).await?;
        let enforce_sni_match = matches!(options.cert_mode, ListenerCertMode::PerHostCaSigned);
        Ok(TlsServerConfig::new().with_dynamic_config(Arc::new(ConnectCertificate {
            name,
            enforce_sni_match,
            config,
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
            fallback_host: normalize_cert_host(&fallback_host),
            observed_sni,
        }))
    }

    pub(crate) async fn resolve_rustls_server_config(
        &self,
        host: &str,
        options: &ListenerTlsOptions,
    ) -> Result<Arc<ServerConfig>, ProxyError> {
        let name = normalize_cert_host(host);
        let cache_identity = match &options.cert_mode {
            ListenerCertMode::PerHostCaSigned => format!("ca:{name}"),
            ListenerCertMode::SelfSigned => {
                let effective = if name.is_empty() { "localhost" } else { &name };
                format!("self:{effective}")
            }
            ListenerCertMode::SpecificHostname(specific) => {
                let normalized = normalize_cert_host(specific);
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
                        LeafCertRequest::new(identity),
                    ))
                    .map_err(ProxyError::tls)?
                }
                ListenerCertMode::SpecificHostname(specific) => {
                    let normalized = normalize_cert_host(&specific);
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

fn normalize_cert_host(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn der_to_pem(der: &[u8], label: &str) -> String {
    let encoded = STANDARD.encode(der);
    let mut pem = String::with_capacity(encoded.len() + label.len() * 2 + 48);
    pem.push_str("-----BEGIN ");
    pem.push_str(label);
    pem.push_str("-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        pem.push('\n');
    }
    pem.push_str("-----END ");
    pem.push_str(label);
    pem.push_str("-----\n");
    pem
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
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ProxyError::Tls(format!("PEM 证书解析失败：{err}")))?;
    if certs.is_empty() {
        return Err(ProxyError::Tls("未找到 PEM 证书".into()));
    }
    if certs.len() > MAX_CUSTOM_CERT_CHAIN_LEN {
        return Err(ProxyError::Tls(format!(
            "PEM 证书链超过 {MAX_CUSTOM_CERT_CHAIN_LEN} 张上限"
        )));
    }
    let mut keys: Vec<PrivateKeyDer<'static>> = PrivateKeyDer::pem_slice_iter(key_bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ProxyError::Tls(format!("PEM 私钥解析失败：{err}")))?;
    if keys.is_empty() {
        return Err(ProxyError::Tls("未找到受支持的未加密 PEM 私钥".into()));
    }
    if keys.len() > 1 {
        return Err(ProxyError::Tls(
            "PEM 材料包含多把私钥，请仅提供一把私钥".into(),
        ));
    }
    Ok((certs, keys.pop().expect("checked single key")))
}

fn issue_ca_signed_leaf(
    issuer: &CertificateAuthorityData,
    normalized_host: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ProxyError> {
    let host: Host = normalized_host.parse().map_err(ProxyError::tls)?;
    let identity = CertificateIdentity::try_from_host(&host).map_err(ProxyError::tls)?;
    let (mut chain, key) = issuer
        .issue_leaf(LeafCertRequest::new(identity))
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
}

impl DynamicConfigProvider for ConnectCertificate {
    async fn get_config(
        &self,
        hello: rustls::server::ClientHello<'_>,
    ) -> Result<Arc<ServerConfig>, rama::error::BoxError> {
        if self.enforce_sni_match
            && let Some(sni) = hello.server_name()
        {
            let normalized_sni = sni.trim_end_matches('.');
            let target_is_ip = self.name.parse::<std::net::IpAddr>().is_ok();
            if !target_is_ip && !normalized_sni.eq_ignore_ascii_case(&self.name) {
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
            .map(normalize_cert_host)
            .filter(|name| !name.is_empty());
        if let Some(ref host) = sni {
            *self
                .observed_sni
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host.clone());
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

fn build_rustls_client_config(
    trust: &UpstreamTls,
    alpn: ClientAlpnMode,
    options: &UpstreamTlsOptions,
    client_identity: Option<&crate::ParsedClientIdentity>,
) -> Result<Arc<rustls::ClientConfig>, ProxyError> {
    let supported_versions = options.resolved_protocol_versions()?;
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(shared_crypto_provider()));
    let builder = match supported_versions.as_deref() {
        None => builder.with_safe_default_protocol_versions(),
        Some(versions) => {
            let rustls_versions: Vec<&'static rustls::SupportedProtocolVersion> = versions
                .iter()
                .filter_map(|v| match *v {
                    ProtocolVersion::TLSv1_3 => Some(&rustls::version::TLS13),
                    ProtocolVersion::TLSv1_2 => Some(&rustls::version::TLS12),
                    _ => None,
                })
                .collect();
            builder.with_protocol_versions(&rustls_versions)
        }
    }
    .map_err(ProxyError::tls)?;

    let builder = match trust {
        UpstreamTls::System => {
            builder.with_root_certificates(Arc::clone(shared_system_root_certs()))
        }
        UpstreamTls::Insecure => {
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoServerCertVerifier::new()))
        }
        UpstreamTls::CustomRoots {
            certificates_der,
            include_system,
        } => {
            let mut roots = if *include_system {
                shared_system_root_certs().as_ref().clone()
            } else {
                rustls::RootCertStore::empty()
            };
            for (index, certificate) in certificates_der.iter().enumerate() {
                roots
                    .add(CertificateDer::from(certificate.as_slice()))
                    .map_err(|error| {
                        ProxyError::Tls(format!(
                            "第 {} 个自定义根证书 DER 无效：{error}",
                            index + 1
                        ))
                    })?;
            }
            builder.with_root_certificates(Arc::new(roots))
        }
    };

    let mut rustls_config = match client_identity {
        Some(identity) => builder
            .with_client_auth_cert(identity.certificate_chain().to_vec(), identity.private_key())
            .map_err(ProxyError::tls)?,
        None => builder.with_no_client_auth(),
    };
    rustls_config.alpn_protocols = match alpn {
        ClientAlpnMode::Http2 => vec![b"h2".to_vec()],
        ClientAlpnMode::Http1 => vec![b"http/1.1".to_vec()],
        ClientAlpnMode::Auto => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
    };
    if options.disable_session_resume {
        rustls_config.resumption = rustls::client::Resumption::disabled();
    }
    Ok(Arc::new(rustls_config))
}

fn wrap_cached_rustls_client_config(
    rustls_config: Arc<rustls::ClientConfig>,
    alpn: ClientAlpnMode,
    store_server_cert_chain: bool,
) -> TlsClientConfig {
    let mut config = match alpn {
        ClientAlpnMode::Http2 => TlsClientConfig::new().with_alpn_http_2(),
        ClientAlpnMode::Http1 => TlsClientConfig::new().with_alpn_http_1(),
        ClientAlpnMode::Auto => TlsClientConfig::new().with_alpn_http_auto(),
    }
    .with_keylog(KeyLogIntent::Disabled)
    .with_server_verify(ServerVerifyMode::Disable)
    .with_store_server_cert_chain(store_server_cert_chain);
    config = config.with_modify_rustls_config(move |incoming| {
        let mut cfg = (*rustls_config).clone();
        if !incoming.alpn_protocols.is_empty() && cfg.alpn_protocols != incoming.alpn_protocols {
            cfg.alpn_protocols = incoming.alpn_protocols;
        }
        if !cfg.client_auth_cert_resolver.has_certs()
            && incoming.client_auth_cert_resolver.has_certs()
        {
            cfg.client_auth_cert_resolver = incoming.client_auth_cert_resolver;
        }
        Ok(cfg)
    });
    config
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
                if certificates_der.is_empty() || certificates_der.len() > 32 {
                    return Err(ProxyError::Tls(
                        "自定义信任根列表须包含 1 到 32 个证书".into(),
                    ));
                }
                for certificate in certificates_der {
                    if certificate.is_empty() || certificate.len() > 64 * 1024 {
                        return Err(ProxyError::Tls(
                            "自定义根证书须为非空且不超过 64 KiB 的 DER 编码".into(),
                        ));
                    }
                }
                TrustModeKey::CustomRoots {
                    include_system: *include_system,
                    certs_sha256: hash_cert_ders(certificates_der),
                }
            }
        };
        let key = ClientTlsCacheKey {
            trust: trust_key,
            alpn,
            options: *options,
            client_cert_sha256: client_identity.map(hash_client_identity),
            store_server_cert_chain,
        };
        {
            let mut cache = client_tls_cache()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(index) = cache.iter().position(|entry| entry.key == key) {
                let entry = cache.remove(index).expect("position came from this cache");
                let config = entry.tls_config.clone();
                cache.push_back(entry);
                return Ok((config, key));
            }
        }
        let rustls_config = build_rustls_client_config(self, alpn, options, client_identity)?;
        let tls_config =
            wrap_cached_rustls_client_config(rustls_config, alpn, store_server_cert_chain);
        {
            let mut cache = client_tls_cache()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    fn ca_regenerate_and_pem_pair_import_roundtrip() {
        let first = MitmAuthority::generate().unwrap();
        let regenerated = MitmAuthority::regenerate().unwrap();
        assert_ne!(first.certificate_der(), regenerated.certificate_der());

        let cert_pem = first.certificate_pem();
        let key_pem = first.private_key_pem();
        let imported = MitmAuthority::from_pem_pair(&cert_pem, &key_pem).unwrap();
        assert_eq!(imported.certificate_der(), first.certificate_der());
        assert_eq!(imported.private_key_der(), first.private_key_der());

        let combined = format!("{cert_pem}\n{key_pem}");
        let imported_combined = MitmAuthority::from_pem_pair(&combined, "").unwrap();
        assert_eq!(imported_combined.certificate_der(), first.certificate_der());

        assert!(MitmAuthority::from_pem_pair(&cert_pem, &regenerated.private_key_pem()).is_err());
        assert!(MitmAuthority::from_pem_pair("not a pem", &key_pem).is_err());
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
                CertificateIdentity::try_from_host(&"custom.example".parse::<Host>().unwrap())
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
        };
        assert!(UpstreamTls::System.config_with_options(None, &invalid).is_err());
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
}
