use anyhow::{Context, Result, ensure};

mod digests;

use super::*;
use digests::{
    base64_url_decode_bytes, base64_url_encode_bytes, compile_rule_regexes, gzip_compress_bytes,
    html_decode_bytes, html_encode_bytes, md5_hex_bytes, reverse_payload_bytes, sha1_hex_bytes,
    sha256_hex_bytes, sha384_hex_bytes, sha512_hex_bytes, substring_slice, transform_case_bytes,
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
    if let [rule] = rules {
        let regex = compiled_regexes[0].as_ref();
        let mut output_bytes = 0usize;
        let mut kept_lens = Vec::with_capacity(values.len());
        for value in values {
            if matches!(rule, PayloadProcessingRule::SkipIfMatchesRegex { .. })
                && regex.is_some_and(|re| re.is_match(value))
            {
                kept_lens.push(None);
                continue;
            }
            let len = payload_processing_output_len(value, rule, regex)?;
            output_bytes = output_bytes
                .checked_add(len)
                .context("Intruder payload 总字节数溢出")?;
            ensure_payload_byte_limit(output_bytes, limit)?;
            kept_lens.push(Some(len));
        }
        let mut out = Vec::with_capacity(values.len());
        for (value, maybe_len) in values.iter().zip(kept_lens) {
            if let Some(len) = maybe_len {
                out.push(transform_payload(value, rule, regex, len)?);
            }
        }
        return Ok(out);
    }
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
            let output_len = payload_processing_output_len(source, rule, regex.as_ref())?;
            *total_bytes = total_bytes
                .checked_add(output_len)
                .context("Intruder payload 总字节数溢出")?;
            ensure_payload_byte_limit(*total_bytes, limit)?;
            transformed = Some(transform_payload(source, rule, regex.as_ref(), output_len)?);
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

fn payload_processing_output_len(
    input: &[u8],
    rule: &PayloadProcessingRule,
    regex: Option<&regex::bytes::Regex>,
) -> Result<usize> {
    let length = match rule {
        PayloadProcessingRule::AddPrefix(text) | PayloadProcessingRule::AddSuffix(text) => {
            input.len().checked_add(text.len())
        }
        PayloadProcessingRule::Uppercase => Some(transform_case_bytes(input, true).len()),
        PayloadProcessingRule::Lowercase => Some(transform_case_bytes(input, false).len()),
        PayloadProcessingRule::Substring { offset, length } => {
            Some(substring_slice(input, *offset, *length).len())
        }
        PayloadProcessingRule::Reverse | PayloadProcessingRule::SkipIfMatchesRegex { .. } => {
            Some(input.len())
        }
        PayloadProcessingRule::Sha1Hex => Some(40),
        PayloadProcessingRule::Sha256Hex => Some(64),
        PayloadProcessingRule::Sha384Hex => Some(96),
        PayloadProcessingRule::Sha512Hex => Some(128),
        PayloadProcessingRule::Md5Hex => Some(32),
        PayloadProcessingRule::HtmlEncode => Some(html_encode_bytes(input).len()),
        PayloadProcessingRule::HtmlDecode => Some(html_decode_bytes(input).len()),
        PayloadProcessingRule::Base64UrlEncode => Some(base64_url_encode_bytes(input).len()),
        PayloadProcessingRule::Base64UrlDecode => Some(base64_url_decode_bytes(input)?.len()),
        PayloadProcessingRule::GzipCompress => Some(gzip_compress_bytes(input)?.len()),
        PayloadProcessingRule::InvokeExtension { processor_name } => {
            anyhow::bail!("扩展处理器 {processor_name} 需要通过已加载插件通道运行")
        }
        PayloadProcessingRule::AddIfNotMatchRegex { addition, .. } => {
            if regex.is_some_and(|re| re.is_match(input)) {
                Some(input.len())
            } else {
                input.len().checked_add(addition.len())
            }
        }
        PayloadProcessingRule::MatchReplaceRegex { replace_with, .. } => {
            let re = regex.context("MatchReplaceRegex 缺少已编译正则表达式")?;
            let replaced = re.replace_all(input, replace_with.as_bytes());
            Some(replaced.len())
        }
        PayloadProcessingRule::MatchReplace {
            match_pattern,
            replace_with,
        } => {
            let count = literal_match_offsets(input, match_pattern.as_bytes()).count();
            input
                .len()
                .checked_sub(
                    count
                        .checked_mul(match_pattern.len())
                        .context("匹配长度溢出")?,
                )
                .and_then(|remaining| remaining.checked_add(count.checked_mul(replace_with.len())?))
        }
        PayloadProcessingRule::UrlEncode => input.len().checked_add(
            input
                .iter()
                .filter(|&&byte| !url_unreserved(byte))
                .count()
                .checked_mul(2)
                .context("URL 编码长度溢出")?,
        ),
        PayloadProcessingRule::UrlDecode => {
            Some(url_decoded_bytes(input).try_fold(0usize, |length, byte| {
                byte?;
                Ok::<_, anyhow::Error>(length + 1)
            })?)
        }
        PayloadProcessingRule::Base64Encode => input.len().div_ceil(3).checked_mul(4),
        PayloadProcessingRule::Base64Decode => {
            ensure!(
                input.len().is_multiple_of(4),
                "Base64 输入长度必须为 4 的倍数"
            );
            let (chunks, _) = input.as_chunks::<4>();
            let Some((last, prefix)) = chunks.split_last() else {
                return Ok(0);
            };
            ensure!(
                prefix
                    .iter()
                    .flatten()
                    .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/')),
                "Base64 包含无效字符或非末尾填充"
            );
            let (_, tail_length) = decode_base64_quartet(last)?;
            prefix
                .len()
                .checked_mul(3)
                .and_then(|length| length.checked_add(tail_length))
        }
        PayloadProcessingRule::AsciiHexEncode => input.len().checked_mul(2),
        PayloadProcessingRule::AsciiHexDecode => {
            let mut digits = 0usize;
            for &byte in input {
                if !byte.is_ascii_whitespace() {
                    hex_nibble(byte).context("ASCII Hex 解码包含非十六进制字符")?;
                    digits += 1;
                }
            }
            ensure!(
                digits.is_multiple_of(2),
                "ASCII Hex 解码十六进制数字个数必须为偶数"
            );
            Some(digits / 2)
        }
    };
    length.context("Intruder payload 总字节数溢出")
}

fn url_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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
                let hi = hex_nibble(hi).context("URL 解码包含无效十六进制字符")?;
                let lo = hex_nibble(lo).context("URL 解码包含无效十六进制字符")?;
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
    output_len: usize,
) -> Result<Vec<u8>> {
    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(output_len);
    match rule {
        PayloadProcessingRule::AddPrefix(prefix) => {
            out.extend_from_slice(prefix.as_bytes());
            out.extend_from_slice(input);
        }
        PayloadProcessingRule::AddSuffix(suffix) => {
            out.extend_from_slice(input);
            out.extend_from_slice(suffix.as_bytes());
        }
        PayloadProcessingRule::Uppercase => return Ok(transform_case_bytes(input, true)),
        PayloadProcessingRule::Lowercase => return Ok(transform_case_bytes(input, false)),
        PayloadProcessingRule::Substring { offset, length } => {
            return Ok(substring_slice(input, *offset, *length));
        }
        PayloadProcessingRule::Reverse => return Ok(reverse_payload_bytes(input)),
        PayloadProcessingRule::Sha1Hex => return Ok(sha1_hex_bytes(input)),
        PayloadProcessingRule::Sha256Hex => return Ok(sha256_hex_bytes(input)),
        PayloadProcessingRule::Sha384Hex => return Ok(sha384_hex_bytes(input)),
        PayloadProcessingRule::Sha512Hex => return Ok(sha512_hex_bytes(input)),
        PayloadProcessingRule::Md5Hex => return Ok(md5_hex_bytes(input)),
        PayloadProcessingRule::HtmlEncode => return Ok(html_encode_bytes(input)),
        PayloadProcessingRule::HtmlDecode => return Ok(html_decode_bytes(input)),
        PayloadProcessingRule::Base64UrlEncode => return Ok(base64_url_encode_bytes(input)),
        PayloadProcessingRule::Base64UrlDecode => return base64_url_decode_bytes(input),
        PayloadProcessingRule::GzipCompress => return gzip_compress_bytes(input),
        PayloadProcessingRule::InvokeExtension { processor_name } => {
            anyhow::bail!("扩展处理器 {processor_name} 需要通过已加载插件通道运行")
        }
        PayloadProcessingRule::MatchReplaceRegex { replace_with, .. } => {
            let re = regex.context("MatchReplaceRegex 缺少已编译正则表达式")?;
            return Ok(re.replace_all(input, replace_with.as_bytes()).into_owned());
        }
        PayloadProcessingRule::AddIfNotMatchRegex { addition, .. } => {
            out.extend_from_slice(input);
            if !regex.is_some_and(|re| re.is_match(input)) {
                out.extend_from_slice(addition.as_bytes());
            }
        }
        PayloadProcessingRule::SkipIfMatchesRegex { .. } => {
            out.extend_from_slice(input);
        }
        PayloadProcessingRule::MatchReplace {
            match_pattern,
            replace_with,
        } => {
            let mut copied = 0;
            for offset in literal_match_offsets(input, match_pattern.as_bytes()) {
                out.extend_from_slice(&input[copied..offset]);
                out.extend_from_slice(replace_with.as_bytes());
                copied = offset + match_pattern.len();
            }
            out.extend_from_slice(&input[copied..]);
        }
        PayloadProcessingRule::UrlEncode => {
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
        }
        PayloadProcessingRule::UrlDecode => {
            for byte in url_decoded_bytes(input) {
                out.push(byte?);
            }
        }
        PayloadProcessingRule::Base64Encode => {
            const B64: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            for chunk in input.chunks(3) {
                let b0 = chunk[0];
                let b1 = *chunk.get(1).unwrap_or(&0);
                let b2 = *chunk.get(2).unwrap_or(&0);
                out.push(B64[usize::from(b0 >> 2)]);
                out.push(B64[usize::from(((b0 & 0x03) << 4) | (b1 >> 4))]);
                out.push(if chunk.len() > 1 {
                    B64[usize::from(((b1 & 0x0f) << 2) | (b2 >> 6))]
                } else {
                    b'='
                });
                out.push(if chunk.len() > 2 {
                    B64[usize::from(b2 & 0x3f)]
                } else {
                    b'='
                });
            }
        }
        PayloadProcessingRule::Base64Decode => {
            for chunk in input.as_chunks::<4>().0 {
                let (bytes, len) = decode_base64_quartet(chunk)?;
                out.extend_from_slice(&bytes[..len]);
            }
        }
        PayloadProcessingRule::AsciiHexEncode => {
            for &byte in input {
                out.extend_from_slice(&[
                    HEX_LOWER[usize::from(byte >> 4)],
                    HEX_LOWER[usize::from(byte & 0x0f)],
                ]);
            }
        }
        PayloadProcessingRule::AsciiHexDecode => {
            let mut digits = input.iter().filter(|byte| !byte.is_ascii_whitespace());
            while let Some(&hi) = digits.next() {
                let lo = *digits.next().context("ASCII Hex 解码缺少低位")?;
                let hi = hex_nibble(hi).context("ASCII Hex 解码包含非十六进制字符")?;
                let lo = hex_nibble(lo).context("ASCII Hex 解码包含非十六进制字符")?;
                out.push((hi << 4) | lo);
            }
        }
    }
    Ok(out)
}

fn base64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn decode_base64_quartet(chunk: &[u8; 4]) -> Result<([u8; 3], usize)> {
    let pad = chunk.iter().rev().take_while(|&&b| b == b'=').count();
    ensure!(pad <= 2, "Base64 填充字符数量无效");
    ensure!(
        chunk[..4 - pad].iter().all(|&b| b != b'='),
        "Base64 填充字符后不能再出现数据字符"
    );
    let s0 = base64_sextet(chunk[0]).context("Base64 包含无效字符")?;
    let s1 = base64_sextet(chunk[1]).context("Base64 包含无效字符")?;
    let mut out = [(s0 << 2) | (s1 >> 4), 0, 0];
    if pad == 2 {
        ensure!((s1 & 0x0f) == 0, "Base64 双字节填充尾位非零");
        return Ok((out, 1));
    }
    let s2 = base64_sextet(chunk[2]).context("Base64 包含无效字符")?;
    out[1] = (s1 << 4) | (s2 >> 2);
    if pad == 1 {
        ensure!((s2 & 0x03) == 0, "Base64 单字节填充尾位非零");
        return Ok((out, 2));
    }
    let s3 = base64_sextet(chunk[3]).context("Base64 包含无效字符")?;
    out[2] = (s2 << 6) | s3;
    Ok((out, 3))
}
