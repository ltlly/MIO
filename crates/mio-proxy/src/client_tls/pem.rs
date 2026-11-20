use super::*;
use rama::{
    crypto::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    tls::{
        client::{ClientAuth, ClientAuthData, TlsClientConfig},
        rustls::dep::rustls,
    },
};

impl ParsedClientIdentity {
    pub fn parse(rule: &ClientCertificateRule) -> Result<Self, ProxyError> {
        let _ = ClientCertificateRule::validate_host_pattern(&rule.host_pattern)?;
        if rule.certificate_bytes.is_empty() || rule.certificate_bytes.len() > MAX_CLIENT_CERT_BYTES
        {
            return Err(ProxyError::Tls(format!(
                "客户端证书数据须为非空且不超过 {MAX_CLIENT_CERT_BYTES} 字节"
            )));
        }
        if rule.private_key_pem.len() > MAX_CLIENT_CERT_BYTES {
            return Err(ProxyError::Tls(format!(
                "客户端 PEM 私钥数据不能超过 {MAX_CLIENT_CERT_BYTES} 字节"
            )));
        }
        if rule.password.len() > MAX_CLIENT_CERT_PASSWORD_BYTES {
            return Err(ProxyError::invalid(format!(
                "客户端证书口令不能超过 {MAX_CLIENT_CERT_PASSWORD_BYTES} 字节"
            )));
        }
        match rule.format {
            ClientCertificateFormat::Pem => {
                Self::from_pem(&rule.certificate_bytes, &rule.private_key_pem)
            }
            ClientCertificateFormat::Pkcs12 => {
                Self::from_pkcs12(&rule.certificate_bytes, &rule.password)
            }
        }
    }

    pub fn from_pem(certificate_pem: &[u8], private_key_pem: &[u8]) -> Result<Self, ProxyError> {
        if certificate_pem.is_empty() || certificate_pem.len() > MAX_CLIENT_CERT_BYTES {
            return Err(ProxyError::Tls(format!(
                "客户端 PEM 证书数据须为非空且不超过 {MAX_CLIENT_CERT_BYTES} 字节"
            )));
        }
        if private_key_pem.len() > MAX_CLIENT_CERT_BYTES {
            return Err(ProxyError::Tls(format!(
                "客户端 PEM 私钥数据不能超过 {MAX_CLIENT_CERT_BYTES} 字节"
            )));
        }
        let key_source = if private_key_pem.is_empty() {
            certificate_pem
        } else {
            private_key_pem
        };
        let (certs, key) = parse_pem_materials(certificate_pem, key_source)?;
        validate_and_build_identity(certs, key)
    }

    pub fn from_pkcs12(pkcs12_bytes: &[u8], password: &str) -> Result<Self, ProxyError> {
        if pkcs12_bytes.is_empty() || pkcs12_bytes.len() > MAX_CLIENT_CERT_BYTES {
            return Err(ProxyError::Tls(format!(
                "客户端 PKCS#12 数据须为非空且不超过 {MAX_CLIENT_CERT_BYTES} 字节"
            )));
        }
        if password.len() > MAX_CLIENT_CERT_PASSWORD_BYTES
            || password
                .bytes()
                .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
        {
            return Err(ProxyError::invalid("PKCS#12 口令无效或包含换行符／空字节"));
        }
        let pem_output = ZeroizeBytes(unpack_pkcs12_to_pem(pkcs12_bytes, password)?);
        let (mut certs, key) = parse_pem_materials(&pem_output.0, &pem_output.0)?;
        reorder_pkcs12_leaf(&mut certs, &key)?;
        validate_and_build_identity(certs, key)
    }

    pub fn summary(&self) -> &ClientCertificateSummary {
        &self.summary
    }

    pub fn certificate_chain(&self) -> &[CertificateDer<'static>] {
        &self.cert_chain
    }

    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        self.private_key.clone_key()
    }

    pub fn client_auth(&self) -> ClientAuth {
        ClientAuth::Single(ClientAuthData {
            private_key: self.private_key.clone_key(),
            cert_chain: self.cert_chain.clone(),
        })
    }

    pub fn apply_to_tls_config(&self, config: TlsClientConfig) -> TlsClientConfig {
        config.with_client_auth(self.client_auth())
    }
}

fn parse_pem_materials(
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ProxyError> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ProxyError::Tls(format!("客户端 PEM 证书解析失败：{error}")))?;
    if certs.is_empty() {
        return Err(ProxyError::Tls("未找到 PEM 客户端证书".into()));
    }
    if certs.len() > MAX_CLIENT_CERT_CHAIN_LEN {
        return Err(ProxyError::Tls(format!(
            "客户端证书链超过 {MAX_CLIENT_CERT_CHAIN_LEN} 张上限"
        )));
    }
    let mut keys: Vec<PrivateKeyDer<'static>> = PrivateKeyDer::pem_slice_iter(key_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ProxyError::Tls(format!("客户端 PEM 私钥解析失败：{error}")))?;
    if keys.is_empty() {
        return Err(ProxyError::Tls(
            "未找到受支持的未加密 PEM 客户端私钥".into(),
        ));
    }
    if keys.len() > 1 {
        return Err(ProxyError::Tls(
            "客户端证书材料包含多把私钥，未猜测使用哪一把".into(),
        ));
    }
    Ok((certs, keys.pop().expect("checked single key")))
}

fn reorder_pkcs12_leaf(
    certs: &mut Vec<CertificateDer<'static>>,
    private_key: &PrivateKeyDer<'static>,
) -> Result<(), ProxyError> {
    if certs.len() <= 1 {
        return Ok(());
    }
    let provider = rustls::crypto::ring::default_provider();
    let signing_key = provider
        .key_provider
        .load_private_key(private_key.clone_key())
        .map_err(|error| ProxyError::Tls(format!("客户端私钥不受支持或已损坏：{error}")))?;
    if rustls::sign::CertifiedKey::new(vec![certs[0].clone()], signing_key.clone())
        .keys_match()
        .is_ok()
    {
        return Ok(());
    }
    if let Some(index) = (1..certs.len()).find(|&idx| {
        rustls::sign::CertifiedKey::new(vec![certs[idx].clone()], signing_key.clone())
            .keys_match()
            .is_ok()
    }) {
        let leaf = certs.remove(index);
        certs.insert(0, leaf);
    }
    Ok(())
}

fn validate_and_build_identity(
    certs: Vec<CertificateDer<'static>>,
    private_key: PrivateKeyDer<'static>,
) -> Result<ParsedClientIdentity, ProxyError> {
    let mut subject_hint = None;
    for (index, cert) in certs.iter().enumerate() {
        let (remaining, parsed) =
            rama::crypto::dep::x509_parser::parse_x509_certificate(cert.as_ref()).map_err(
                |error| ProxyError::Tls(format!("第 {} 张客户端证书 DER 无效：{error}", index + 1)),
            )?;
        if !remaining.is_empty() {
            return Err(ProxyError::Tls(format!(
                "第 {} 张客户端证书包含额外尾随字节",
                index + 1
            )));
        }
        if index == 0 {
            let hint = if let Some(cn) = parsed.subject().iter_common_name().next() {
                cn.as_str()
                    .map(str::to_owned)
                    .map_err(|error| ProxyError::Tls(format!("客户端证书 CN 无法解码：{error}")))?
            } else {
                parsed.subject().to_string()
            };
            subject_hint = Some(if hint.is_empty() {
                "客户端证书".into()
            } else {
                hint
            });
        }
    }

    let provider = rustls::crypto::ring::default_provider();
    let certified_key =
        rustls::sign::CertifiedKey::from_der(certs.clone(), private_key.clone_key(), &provider)
            .map_err(|error| ProxyError::Tls(format!("客户端证书与私钥加载失败：{error}")))?;
    certified_key
        .keys_match()
        .map_err(|error| ProxyError::Tls(format!("客户端证书公钥与私钥不匹配：{error}")))?;
    let _ = rustls::sign::SingleCertAndKey::from(certified_key);

    let cert_count = certs.len();
    Ok(ParsedClientIdentity {
        cert_chain: certs,
        private_key,
        summary: ClientCertificateSummary {
            subject_hint: subject_hint.unwrap_or_else(|| "客户端证书".into()),
            cert_count,
        },
    })
}
