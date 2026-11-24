use anyhow::{Context, Result, ensure};

mod digests;

use super::*;
use crate::codec_util;
use digests::{
    compile_rule_regexes, gzip_compress_bytes, reverse_payload_bytes, substring_slice,
    transform_case_bytes,
};

pub const DEFAULT_PAYLOAD_ENCODING_CHARS: &str = "!#$&'()*+,/:;=?@[] ";

pub fn percent_encode_selected_chars(input: &[u8], chars_to_encode: &str) -> Vec<u8> {
    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    let mask = chars_to_encode.as_bytes();
    if mask.is_empty() {
        return input.to_vec();
    }
    let mut table = [false; 256];
    for &byte in mask {
        table[usize::from(byte)] = true;
    }
    let extra = input
        .iter()
        .filter(|&&byte| table[usize::from(byte)])
        .count();
    let mut out = Vec::with_capacity(input.len().saturating_add(extra.saturating_mul(2)));
    for &byte in input {
        if table[usize::from(byte)] {
            out.extend_from_slice(&[
                b'%',
                HEX_UPPER[usize::from(byte >> 4)],
                HEX_UPPER[usize::from(byte & 0x0f)],
            ]);
        } else {
            out.push(byte);
        }
    }
    out
}

pub fn generate_number_payloads(
    from: i64,
    to: i64,
    step: i64,
    min_integer_digits: usize,
    hex: bool,
) -> Result<Vec<Vec<u8>>> {
    ensure!(step > 0, "步长必须为正整数");
    ensure!(
        (1..=32).contains(&min_integer_digits),
        "最少整数位数必须在 1..=32 之间"
    );
    let start = i128::from(from);
    let end = i128::from(to);
    let step_i128 = i128::from(step);
    let span = (end - start).unsigned_abs();
    let steps = span / step_i128.unsigned_abs();
    let count_u128 = steps + 1;
    ensure!(
        count_u128 <= u128::try_from(MAX_PAYLOADS)?,
        "生成数字数量超过 {MAX_PAYLOADS} 上限"
    );
    let count = usize::try_from(count_u128)?;
    let max_magnitude = start.unsigned_abs().max(end.unsigned_abs());
    let max_digits = if max_magnitude == 0 {
        1
    } else if hex {
        usize::try_from(max_magnitude.ilog2())? / 4 + 1
    } else {
        usize::try_from(max_magnitude.ilog10())? + 1
    };
    let max_value_bytes = min_integer_digits.max(max_digits) + usize::from(from < 0 || to < 0);
    ensure_payload_byte_limit(
        count
            .checked_mul(max_value_bytes)
            .context("Intruder payload 总字节数溢出")?,
        MAX_PAYLOAD_BYTES,
    )?;
    let mut out = Vec::with_capacity(count);
    let direction = if from <= to { 1i128 } else { -1i128 };
    for index in 0..count {
        let value = start + (i128::try_from(index)? * step_i128 * direction);
        let magnitude = value.unsigned_abs();
        let formatted = match (value < 0, hex) {
            (false, false) => format!("{magnitude:0width$}", width = min_integer_digits),
            (true, false) => format!("-{magnitude:0width$}", width = min_integer_digits),
            (false, true) => format!("{magnitude:0width$X}", width = min_integer_digits),
            (true, true) => format!("-{magnitude:0width$X}", width = min_integer_digits),
        };
        out.push(formatted.into_bytes());
    }
    Ok(out)
}

pub fn apply_payload_processing_rule(
    values: &[Vec<u8>],
    rule: &PayloadProcessingRule,
) -> Result<Vec<Vec<u8>>> {
    apply_payload_processing_rules(values, std::slice::from_ref(rule))
}

pub fn apply_payload_processing_rules(
    values: &[Vec<u8>],
    rules: &[PayloadProcessingRule],
) -> Result<Vec<Vec<u8>>> {
    apply_payload_processing_rules_with_limit(values, rules, MAX_PAYLOAD_BYTES)
}

fn ensure_payload_byte_limit(total_bytes: usize, limit: usize) -> Result<()> {
    ensure!(
        total_bytes <= limit,
        "Intruder payload 总字节数超过 {limit}（MIO 资源限额，未截断）"
    );
    Ok(())
}

pub(super) fn apply_payload_processing_rules_with_limit(
    values: &[Vec<u8>],
    rules: &[PayloadProcessingRule],
    limit: usize,
) -> Result<Vec<Vec<u8>>> {
    ensure!(
        values.len() <= MAX_PAYLOADS,
        "Intruder payload 总数量超过 {MAX_PAYLOADS}（MIO 资源限额，未截断）"
    );
    let mut input_bytes = 0usize;
    for value in values {
        input_bytes = input_bytes
            .checked_add(value.len())
            .context("Intruder payload 总字节数溢出")?;
        ensure_payload_byte_limit(input_bytes, limit)?;
    }
    for rule in rules {
        if let PayloadProcessingRule::MatchReplace { match_pattern, .. } = rule {
            ensure!(!match_pattern.is_empty(), "Match/Replace 匹配模式不能为空");
        }
    }
    let compiled_regexes = compile_rule_regexes(rules)?;
    let mut stage_bytes = vec![0usize; rules.len()];
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let mut transformed: Option<Vec<u8>> = None;
        let mut skipped = false;
        for ((rule, regex), total_bytes) in
            rules.iter().zip(&compiled_regexes).zip(&mut stage_bytes)
        {
            let source = transformed.as_deref().unwrap_or(value);
            if matches!(rule, PayloadProcessingRule::SkipIfMatchesRegex { .. })
                && regex.as_ref().is_some_and(|re| re.is_match(source))
            {
                skipped = true;
                break;
            }
            let next = transform_payload(source, rule, regex.as_ref(), *total_bytes, limit)?;
            *total_bytes = total_bytes
                .checked_add(next.len())
                .context("Intruder payload 总字节数溢出")?;
            ensure_payload_byte_limit(*total_bytes, limit)?;
            transformed = Some(next);
        }
        if !skipped {
            out.push(transformed.unwrap_or_else(|| value.clone()));
        }
    }
    Ok(out)
}

fn literal_match_offsets<'a>(
    input: &'a [u8],
    pattern: &'a [u8],
) -> impl Iterator<Item = usize> + 'a {
    let mut offset = 0;
    std::iter::from_fn(move || {
        let relative = input
            .get(offset..)?
            .windows(pattern.len())
            .position(|window| window == pattern)?;
        let found = offset + relative;
        offset = found + pattern.len();
        Some(found)
    })
}

fn ensure_projected_len(
    stage_bytes: usize,
    projected: Option<usize>,
    limit: usize,
) -> Result<usize> {
    let len = projected.context("Intruder payload 总字节数溢出")?;
    let next_total = stage_bytes
        .checked_add(len)
        .context("Intruder payload 总字节数溢出")?;
    ensure_payload_byte_limit(next_total, limit)?;
    Ok(len)
}

fn url_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

fn url_decoded_bytes(input: &[u8]) -> impl Iterator<Item = Result<u8>> + '_ {
    let mut index = 0;
    std::iter::from_fn(move || {
        let byte = *input.get(index)?;
        index += 1;
        Some(match byte {
            b'+' => Ok(b' '),
            b'%' => (|| {
                let hi = *input.get(index).context("URL 解码遇到不完整的 % 转义")?;
                let lo = *input
                    .get(index + 1)
                    .context("URL 解码遇到不完整的 % 转义")?;
                index += 2;
                let hi = codec_util::hex_nibble(hi).context("URL 解码包含无效十六进制字符")?;
                let lo = codec_util::hex_nibble(lo).context("URL 解码包含无效十六进制字符")?;
                Ok((hi << 4) | lo)
            })(),
            byte => Ok(byte),
        })
    })
}

fn transform_payload(
    input: &[u8],
    rule: &PayloadProcessingRule,
    regex: Option<&regex::bytes::Regex>,
    stage_bytes: usize,
    limit: usize,
) -> Result<Vec<u8>> {
    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    match rule {
        PayloadProcessingRule::AddPrefix(prefix) => {
            let out_len = ensure_projected_len(stage_bytes, input.len().checked_add(prefix.len()), limit)?;
            let mut out = Vec::with_capacity(out_len);
            out.extend_from_slice(prefix.as_bytes());
            out.extend_from_slice(input);
            Ok(out)
        }
        PayloadProcessingRule::AddSuffix(suffix) => {
            let out_len = ensure_projected_len(stage_bytes, input.len().checked_add(suffix.len()), limit)?;
            let mut out = Vec::with_capacity(out_len);
            out.extend_from_slice(input);
            out.extend_from_slice(suffix.as_bytes());
            Ok(out)
        }
        PayloadProcessingRule::Uppercase => Ok(transform_case_bytes(input, true)),
        PayloadProcessingRule::Lowercase => Ok(transform_case_bytes(input, false)),
        PayloadProcessingRule::Substring { offset, length } => {
            Ok(substring_slice(input, *offset, *length))
        }
        PayloadProcessingRule::Reverse => Ok(reverse_payload_bytes(input)),
        PayloadProcessingRule::Sha1Hex => {
            ensure_projected_len(stage_bytes, Some(40), limit)?;
            Ok(codec_util::hex_encode(&codec_util::sha1_digest(input)).into_bytes())
        }
        PayloadProcessingRule::Sha256Hex => {
            ensure_projected_len(stage_bytes, Some(64), limit)?;
            Ok(codec_util::hex_encode(&codec_util::sha256_digest(input)).into_bytes())
        }
        PayloadProcessingRule::Sha384Hex => {
            ensure_projected_len(stage_bytes, Some(96), limit)?;
            Ok(codec_util::hex_encode(&codec_util::sha384_digest(input)).into_bytes())
        }
        PayloadProcessingRule::Sha512Hex => {
            ensure_projected_len(stage_bytes, Some(128), limit)?;
            Ok(codec_util::hex_encode(&codec_util::sha512_digest(input)).into_bytes())
        }
        PayloadProcessingRule::Md5Hex => {
            ensure_projected_len(stage_bytes, Some(32), limit)?;
            Ok(codec_util::hex_encode(&codec_util::md5_digest(input)).into_bytes())
        }
        PayloadProcessingRule::HtmlEncode => {
            let projected = input.iter().try_fold(0usize, |acc, &b| {
                let add = match b {
                    b'&' => 5,
                    b'<' | b'>' => 4,
                    b'"' => 6,
                    b'\'' => 5,
                    _ => 1,
                };
                acc.checked_add(add)
            });
            ensure_projected_len(stage_bytes, projected, limit)?;
            Ok(codec_util::html_encode_bytes(input))
        }
        PayloadProcessingRule::HtmlDecode => Ok(codec_util::html_decode_bytes(input)),
        PayloadProcessingRule::Base64UrlEncode => {
            let projected = input
                .len()
                .checked_mul(4)
                .and_then(|v| v.checked_add(2))
                .map(|v| v / 3);
            ensure_projected_len(stage_bytes, projected, limit)?;
            Ok(codec_util::base64url_encode(input).into_bytes())
        }
        PayloadProcessingRule::Base64UrlDecode => codec_util::base64url_decode(input),
        PayloadProcessingRule::GzipCompress => gzip_compress_bytes(input),
        PayloadProcessingRule::InvokeExtension { processor_name } => {
            anyhow::bail!("扩展处理器 {processor_name} 需要通过已加载插件通道运行")
        }
        PayloadProcessingRule::MatchReplaceRegex { replace_with, .. } => {
            let re = regex.context("MatchReplaceRegex 缺少已编译正则表达式")?;
            Ok(re.replace_all(input, replace_with.as_bytes()).into_owned())
        }
        PayloadProcessingRule::AddIfNotMatchRegex { addition, .. } => {
            let matched = regex.is_some_and(|re| re.is_match(input));
            let projected = if matched {
                Some(input.len())
            } else {
                input.len().checked_add(addition.len())
            };
            let out_len = ensure_projected_len(stage_bytes, projected, limit)?;
            let mut out = Vec::with_capacity(out_len);
            out.extend_from_slice(input);
            if !matched {
                out.extend_from_slice(addition.as_bytes());
            }
            Ok(out)
        }
        PayloadProcessingRule::SkipIfMatchesRegex { .. } => Ok(input.to_vec()),
        PayloadProcessingRule::MatchReplace {
            match_pattern,
            replace_with,
        } => {
            let count = literal_match_offsets(input, match_pattern.as_bytes()).count();
            let projected = input
                .len()
                .checked_sub(
                    count
                        .checked_mul(match_pattern.len())
                        .context("匹配长度溢出")?,
                )
                .and_then(|remaining| remaining.checked_add(count.checked_mul(replace_with.len())?));
            let out_len = ensure_projected_len(stage_bytes, projected, limit)?;
            let mut out = Vec::with_capacity(out_len);
            let mut copied = 0;
            for offset in literal_match_offsets(input, match_pattern.as_bytes()) {
                out.extend_from_slice(&input[copied..offset]);
                out.extend_from_slice(replace_with.as_bytes());
                copied = offset + match_pattern.len();
            }
            out.extend_from_slice(&input[copied..]);
            Ok(out)
        }
        PayloadProcessingRule::UrlEncode => {
            let extra = input
                .iter()
                .filter(|&&byte| !url_unreserved(byte))
                .count()
                .checked_mul(2)
                .context("URL 编码长度溢出")?;
            let out_len = ensure_projected_len(stage_bytes, input.len().checked_add(extra), limit)?;
            let mut out = Vec::with_capacity(out_len);
            for &byte in input {
                if url_unreserved(byte) {
                    out.push(byte);
                } else {
                    out.extend_from_slice(&[
                        b'%',
                        HEX_UPPER[usize::from(byte >> 4)],
                        HEX_UPPER[usize::from(byte & 0x0f)],
                    ]);
                }
            }
            Ok(out)
        }
        PayloadProcessingRule::UrlDecode => url_decoded_bytes(input).collect(),
        PayloadProcessingRule::Base64Encode => {
            ensure_projected_len(stage_bytes, input.len().div_ceil(3).checked_mul(4), limit)?;
            Ok(codec_util::base64_encode(input).into_bytes())
        }
        PayloadProcessingRule::Base64Decode => codec_util::base64_decode(input),
        PayloadProcessingRule::AsciiHexEncode => {
            ensure_projected_len(stage_bytes, input.len().checked_mul(2), limit)?;
            Ok(codec_util::hex_encode(input).into_bytes())
        }
        PayloadProcessingRule::AsciiHexDecode => codec_util::hex_decode_bytes(input),
    }
}
