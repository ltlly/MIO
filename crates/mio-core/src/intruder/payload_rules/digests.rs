use std::io::Write as IoWrite;

use anyhow::{Context, Result, ensure};
use flate2::{Compression, write::GzEncoder};
use regex::bytes::{Regex, RegexBuilder};

use super::super::{MAX_PAYLOAD_RULE_REGEX_BYTES, PayloadProcessingRule};

pub(super) fn compile_bounded_rule_regex(pattern: &str) -> Result<Regex> {
    ensure!(!pattern.is_empty(), "Payload 处理正则表达式不能为空");
    ensure!(
        pattern.len() <= MAX_PAYLOAD_RULE_REGEX_BYTES,
        "正则表达式超过 {MAX_PAYLOAD_RULE_REGEX_BYTES} 字节上限"
    );
    RegexBuilder::new(pattern)
        .size_limit(MAX_PAYLOAD_RULE_REGEX_BYTES)
        .dfa_size_limit(MAX_PAYLOAD_RULE_REGEX_BYTES)
        .build()
        .context("Payload 处理正则表达式无效或超过 256 KiB 编译限额")
}

pub(super) fn transform_case_bytes(input: &[u8], uppercase: bool) -> Vec<u8> {
    if let Ok(text) = std::str::from_utf8(input) {
        if uppercase {
            text.to_uppercase().into_bytes()
        } else {
            text.to_lowercase().into_bytes()
        }
    } else if uppercase {
        input.to_ascii_uppercase()
    } else {
        input.to_ascii_lowercase()
    }
}

pub(super) fn substring_slice(input: &[u8], offset: usize, length: usize) -> Vec<u8> {
    if let Ok(text) = std::str::from_utf8(input) {
        text.chars()
            .skip(offset)
            .take(length)
            .collect::<String>()
            .into_bytes()
    } else {
        let start = offset.min(input.len());
        let end = start.saturating_add(length).min(input.len());
        input[start..end].to_vec()
    }
}

pub(super) fn reverse_payload_bytes(input: &[u8]) -> Vec<u8> {
    if let Ok(text) = std::str::from_utf8(input) {
        text.chars().rev().collect::<String>().into_bytes()
    } else {
        input.iter().rev().copied().collect()
    }
}

pub(super) fn compile_rule_regexes(rules: &[PayloadProcessingRule]) -> Result<Vec<Option<Regex>>> {
    rules
        .iter()
        .map(|rule| match rule {
            PayloadProcessingRule::AddIfNotMatchRegex { pattern, .. }
            | PayloadProcessingRule::SkipIfMatchesRegex { pattern }
            | PayloadProcessingRule::MatchReplaceRegex { pattern, .. } => {
                compile_bounded_rule_regex(pattern).map(Some)
            }
            _ => Ok(None),
        })
        .collect()
}

pub(super) fn gzip_compress_bytes(input: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .context("Gzip 压缩写入 payload 失败")?;
    encoder.finish().context("Gzip 压缩完成 payload 失败")
}
