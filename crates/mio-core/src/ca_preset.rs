//! Built-in issuing CA presets.
//!
//! A preset carries a complete issuer-first certificate chain and its private
//! key. Selecting a preset imports it through `Command::ImportCa`; the proxy
//! then signs per-host leaves under that CA. Clients whose trust store already
//! contains the preset's trust anchor accept those leaves without installing a
//! MIO CA.

use anyhow::{Result, ensure};

/// A built-in CA preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaPreset {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// SHA-256 fingerprint of the issuing CA certificate (`XX:XX:...` uppercase hex).
    pub fingerprint_sha256: &'static str,
    /// Number of certificates in the embedded issuer-first CA chain.
    pub chain_len: usize,
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
    fingerprint_sha256: "42:C4:AC:CD:12:C4:F5:95:97:92:5E:1A:08:A7:A9:01:02:60:85:43:80:93:08:D5:0E:C4:27:E1:18:BA:14:93",
    chain_len: 3,
    pem: include_str!("../assets/ca-presets/emudhra-emsign-dv-g3b.pem"),
};

/// eMudhra emSign DV TLS CA - G3B under the Sectigo cross-sign
/// (`AAA Certificate Services`).
pub const EMUDHRA_AAA_DV_G3B: CaPreset = CaPreset {
    id: "emudhra-aaa-dv-g3b",
    display_name: "eMudhra emSign DV TLS CA - G3B（AAA 交叉签链）",
    description: "链至 AAA Certificate Services（Comodo）。仅适用于信任库未更新、不做吊销校验的客户端。中间 CA 已撤销。",
    fingerprint_sha256: "45:A8:83:F3:49:AE:DF:D5:F3:A6:49:B5:49:8F:FB:12:17:12:BA:8B:44:8F:3F:2A:91:DB:C5:A7:66:FC:DD:BE",
    chain_len: 2,
    pem: include_str!("../assets/ca-presets/emudhra-aaa-dv-g3b.pem"),
};

pub const BUILTIN_CA_PRESETS: &[CaPreset] = &[EMUDHRA_EMSIGN_DV_G3B, EMUDHRA_AAA_DV_G3B];

pub fn ca_preset(id: &str) -> Option<&'static CaPreset> {
    BUILTIN_CA_PRESETS.iter().find(|preset| preset.id == id)
}

/// Look up a built-in CA preset by its issuing certificate SHA-256 fingerprint.
pub fn ca_preset_by_fingerprint(fingerprint: &str) -> Option<&'static CaPreset> {
    let trimmed = fingerprint.trim();
    BUILTIN_CA_PRESETS
        .iter()
        .find(|preset| preset.fingerprint_sha256.eq_ignore_ascii_case(trimmed))
}

/// Resolve a built-in CA preset ID, returning a uniform error listing available IDs.
pub fn resolve_ca_preset(id: &str) -> Result<&'static CaPreset> {
    let trimmed = id.trim();
    ensure!(!trimmed.is_empty(), "CA 预置 ID 不能为空");
    ca_preset(trimmed).ok_or_else(|| {
        let available = BUILTIN_CA_PRESETS
            .iter()
            .map(|preset| preset.id)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::anyhow!("未知 CA 预置：{trimmed}（可选：{available}）")
    })
}

/// Build IPC metadata entries for every built-in CA preset.
pub fn builtin_ca_preset_infos() -> Vec<crate::CaPresetInfo> {
    BUILTIN_CA_PRESETS
        .iter()
        .map(|preset| crate::CaPresetInfo {
            id: preset.id.to_string(),
            display_name: preset.display_name.to_string(),
            description: preset.description.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{
        fingerprint_sha256, load_if_changed_at, save_authority_with_stamp_at,
    };
    use mio_proxy::MitmAuthority;

    #[test]
    fn preset_ids_and_fingerprints_are_unique_and_resolvable() {
        assert_eq!(BUILTIN_CA_PRESETS.len(), 2);
        let infos = builtin_ca_preset_infos();
        assert_eq!(infos.len(), BUILTIN_CA_PRESETS.len());
        for (preset, info) in BUILTIN_CA_PRESETS.iter().zip(&infos) {
            assert_eq!(ca_preset(preset.id), Some(preset));
            assert_eq!(
                ca_preset_by_fingerprint(preset.fingerprint_sha256),
                Some(preset)
            );
            assert_eq!(
                ca_preset_by_fingerprint(&preset.fingerprint_sha256.to_ascii_lowercase()),
                Some(preset)
            );
            assert_eq!(
                resolve_ca_preset(&format!("  {}  ", preset.id)).unwrap(),
                preset
            );
            assert_eq!(info.id, preset.id);
            assert_eq!(info.display_name, preset.display_name);
            assert_eq!(info.description, preset.description);
            assert!(!preset.display_name.is_empty());
            assert!(!preset.description.is_empty());
        }
        assert!(ca_preset("missing").is_none());
        assert!(ca_preset_by_fingerprint("00:11").is_none());
        assert!(resolve_ca_preset("").is_err());
        assert!(resolve_ca_preset("   ").is_err());
        let missing_err = resolve_ca_preset("missing").unwrap_err().to_string();
        assert!(missing_err.contains("missing"));
        assert!(missing_err.contains("emudhra-emsign-dv-g3b"));
        assert!(missing_err.contains("emudhra-aaa-dv-g3b"));
        assert_eq!(
            BUILTIN_CA_PRESETS
                .iter()
                .map(|preset| preset.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            BUILTIN_CA_PRESETS.len()
        );
        assert_eq!(
            BUILTIN_CA_PRESETS
                .iter()
                .map(|preset| preset.fingerprint_sha256)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            BUILTIN_CA_PRESETS.len()
        );
    }

    #[test]
    fn every_preset_imports_issues_verified_leaf_chain_and_roundtrips_storage() {
        for preset in BUILTIN_CA_PRESETS {
            let authority = MitmAuthority::from_pem_pair(preset.pem, "")
                .unwrap_or_else(|error| panic!("{} 导入失败：{error}", preset.id));
            let chain = authority.certificate_chain_der();
            assert_eq!(chain.len(), preset.chain_len, "{}", preset.id);
            assert_eq!(authority.certificate_der(), chain[0]);
            assert_eq!(
                fingerprint_sha256(&chain[0]),
                preset.fingerprint_sha256,
                "{} 指纹不匹配",
                preset.id
            );
            let spki = authority.spki_sha256_base64().unwrap();
            assert_eq!(spki.len(), 44);

            let leaf_chain = authority
                .issue_leaf_chain_der("lab.example.invalid")
                .unwrap_or_else(|error| panic!("{} 签发 leaf 链失败：{error}", preset.id));
            assert_eq!(leaf_chain.len(), preset.chain_len + 1);
            assert_eq!(&leaf_chain[1..], chain.as_slice());

            let dir = tempfile::tempdir().unwrap();
            let identity_dir = dir.path().join("identity");
            let stamp = save_authority_with_stamp_at(&identity_dir, &authority).unwrap();
            let (reloaded, reloaded_stamp) = load_if_changed_at(&identity_dir, None)
                .unwrap()
                .expect("persisted preset CA must reload cleanly");
            assert_eq!(reloaded_stamp.cert_sha256, stamp.cert_sha256);
            assert_eq!(reloaded.certificate_chain_der(), chain);
            let reloaded_leaf_chain = reloaded
                .issue_leaf_chain_der("lab.example.invalid")
                .unwrap();
            assert_eq!(reloaded_leaf_chain.len(), preset.chain_len + 1);
            assert_eq!(&reloaded_leaf_chain[1..], chain.as_slice());
        }
    }
}
