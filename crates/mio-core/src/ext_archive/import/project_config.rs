pub(crate) mod replace_rules;
pub(crate) mod sections;
pub(crate) mod strict_json;
#[cfg(test)]
mod tests;

use crate::{ExtEmbeddedConfigReport, ExtEmbeddedConfigStatus, ExtEmbeddedProjectConfig};
use sections::extract_sections;
use strict_json::parse_strict_object;

pub const MAX_EMBEDDED_CONFIG_BYTES: usize = 256 * 1024;
const MAX_REPORTED_KEYS: usize = 32;

pub(in crate::ext_archive) const EMBEDDED_CONFIG_PREFIX: &str = "root.f4.f0 (Embedded Project Config): ";

fn invalid_report(reason: impl Into<String>) -> ExtEmbeddedConfigReport {
    ExtEmbeddedConfigReport {
        status: ExtEmbeddedConfigStatus::Invalid {
            reason: reason.into(),
        },
        config: ExtEmbeddedProjectConfig::default(),
        skipped_unsafe_sections: Vec::new(),
        unmapped_keys: Vec::new(),
    }
}

/// Safely extracts verified Ext/MIO project options from a bounded JSON byte slice.
/// Never binds listeners, enables plugins/extensions, or starts automated tasks.
pub fn extract_embedded_ext_project_config(raw_json: &[u8]) -> ExtEmbeddedConfigReport {
    if raw_json.is_empty() {
        return invalid_report("内嵌项目配置 JSON 为空");
    }
    if raw_json.len() > MAX_EMBEDDED_CONFIG_BYTES {
        return invalid_report(format!(
            "内嵌项目配置超过 {} 字节上限；未截断或部分应用",
            MAX_EMBEDDED_CONFIG_BYTES
        ));
    }
    if raw_json.contains(&0) {
        return invalid_report("内嵌项目配置包含空字节");
    }
    let Ok(text) = std::str::from_utf8(raw_json) else {
        return invalid_report("内嵌项目配置不是合法 UTF-8 文本");
    };
    let map = match parse_strict_object(text) {
        Ok(map) => map,
        Err(error) => return invalid_report(format!("内嵌项目配置 JSON 无效：{error}")),
    };
    match extract_sections(&map) {
        Ok((config, skipped_unsafe_sections, mut unmapped_keys)) => {
            if unmapped_keys.len() > MAX_REPORTED_KEYS {
                let omitted = unmapped_keys.len() - MAX_REPORTED_KEYS;
                unmapped_keys.truncate(MAX_REPORTED_KEYS);
                unmapped_keys.push(format!("…其余 {omitted} 项未映射配置键"));
            }
            ExtEmbeddedConfigReport {
                status: ExtEmbeddedConfigStatus::Extracted,
                config,
                skipped_unsafe_sections,
                unmapped_keys,
            }
        }
        Err(error) => invalid_report(format!("{error:#}")),
    }
}
