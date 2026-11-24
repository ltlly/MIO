//! Built-in issuing CA presets.
//!
//! A preset carries a complete issuer-first certificate chain and its private
//! key. Selecting a preset imports it through `Command::ImportCa`; the proxy
//! then signs per-host leaves under that CA. Clients whose trust store already
//! contains the preset's trust anchor accept those leaves without installing a
//! MIO CA.

use anyhow::{Context as _, Result, ensure};
use base64::Engine as _;

/// A built-in CA preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaPreset {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// PEM text: issuer-first certificate chain followed by the private key.
    pub pem: &'static str,
}

/// eMudhra emSign DV TLS CA - G3B under the emSign root chain
/// (`emSign Root TLS CA - G3` cross-signed by `emSign ECC Root CA - G3`).
///
/// From the October 2026 emSign intermediate key compromise. The intermediate
/// is revoked; only clients whose trust store still trusts the emSign roots
/// and that do not enforce revocation accept leaves signed with it.
pub const EMUDHRA_EMSIGN_DV_G3B: CaPreset = CaPreset {
    id: "emudhra-emsign-dv-g3b",
    display_name: "eMudhra emSign DV TLS CA - G3B（emSign 根链）",
    description: "链至 emSign ECC Root CA - G3。仅适用于信任库未更新、不做吊销校验的客户端（例如未升级系统信任库的旧安卓设备）。中间 CA 已撤销。",
    pem: include_str!("../assets/ca-presets/emudhra-emsign-dv-g3b.pem"),
};

/// eMudhra emSign DV TLS CA - G3B under the Sectigo cross-sign
/// (`AAA Certificate Services`).
pub const EMUDHRA_AAA_DV_G3B: CaPreset = CaPreset {
    id: "emudhra-aaa-dv-g3b",
    display_name: "eMudhra emSign DV TLS CA - G3B（AAA 交叉签链）",
    description: "链至 AAA Certificate Services（Comodo）。仅适用于信任库未更新、不做吊销校验的客户端。中间 CA 已撤销。",
    pem: include_str!("../assets/ca-presets/emudhra-aaa-dv-g3b.pem"),
};

pub const BUILTIN_CA_PRESETS: &[CaPreset] = &[EMUDHRA_EMSIGN_DV_G3B, EMUDHRA_AAA_DV_G3B];

pub fn ca_preset(id: &str) -> Option<&'static CaPreset> {
    BUILTIN_CA_PRESETS.iter().find(|preset| preset.id == id)
}

/// Extract the DER encodings of every certificate in a PEM bundle, in order.
pub fn pem_certificate_blocks(pem: &str) -> Result<Vec<Vec<u8>>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in pem.lines() {
        let trimmed = line.trim();
        if trimmed == BEGIN {
            ensure!(current.is_none(), "PEM 证书块嵌套");
            current = Some(String::new());
        } else if trimmed == END {
            let body = current.take().context("PEM 证书块缺少 BEGIN 行")?;
            blocks.push(
                base64::engine::general_purpose::STANDARD
                    .decode(body.as_bytes())
                    .context("PEM 证书块不是有效 base64")?,
            );
        } else if let Some(body) = current.as_mut() {
            ensure!(!trimmed.starts_with("-----"), "PEM 中存在其他块");
            body.push_str(trimmed);
        }
    }
    ensure!(current.is_none(), "PEM 证书块未闭合");
    ensure!(!blocks.is_empty(), "PEM 中未找到证书");
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::fingerprint_sha256;
    use mio_proxy::MitmAuthority;

    #[test]
    fn preset_ids_are_unique_and_resolvable() {
        assert_eq!(BUILTIN_CA_PRESETS.len(), 2);
        for preset in BUILTIN_CA_PRESETS {
            assert_eq!(ca_preset(preset.id), Some(preset));
            assert!(!preset.display_name.is_empty());
            assert!(!preset.description.is_empty());
        }
        assert!(ca_preset("missing").is_none());
        assert_eq!(
            BUILTIN_CA_PRESETS
                .iter()
                .map(|preset| preset.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            BUILTIN_CA_PRESETS.len()
        );
    }

    #[test]
    fn every_preset_imports_as_a_valid_mitm_authority() {
        for preset in BUILTIN_CA_PRESETS {
            let certs = pem_certificate_blocks(preset.pem).unwrap();
            assert!(certs.len() >= 2, "{} 必须带签发者链", preset.id);
            let authority = MitmAuthority::from_pem_pair(preset.pem, "")
                .unwrap_or_else(|error| panic!("{} 导入失败：{error}", preset.id));
            assert_eq!(authority.certificate_chain_der(), certs);
            assert_eq!(authority.certificate_der(), certs[0]);
            assert_eq!(
                fingerprint_sha256(&certs[0]).split(':').count(),
                32,
                "{} 指纹格式错误",
                preset.id
            );
            let spki = authority.spki_sha256_base64().unwrap();
            assert_eq!(spki.len(), 44);
        }
    }

    #[test]
    fn emsign_preset_carries_three_cert_chain() {
        let preset = ca_preset("emudhra-emsign-dv-g3b").unwrap();
        assert_eq!(pem_certificate_blocks(preset.pem).unwrap().len(), 3);
    }

    #[test]
    fn aaa_preset_carries_two_cert_chain() {
        let preset = ca_preset("emudhra-aaa-dv-g3b").unwrap();
        assert_eq!(pem_certificate_blocks(preset.pem).unwrap().len(), 2);
    }

    #[test]
    fn malformed_pem_is_rejected() {
        assert!(pem_certificate_blocks("").is_err());
        assert!(pem_certificate_blocks("-----BEGIN CERTIFICATE-----\n").is_err());
        assert!(pem_certificate_blocks("-----END CERTIFICATE-----\n").is_err());
        assert!(
            pem_certificate_blocks(
                "-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n"
            )
            .is_err()
        );
    }
}
