use super::*;

pub fn extract_token(response: &HttpResponse, location: &SequencerTokenLocation) -> Option<String> {
    match location {
        SequencerTokenLocation::Cookie { name } => extract_cookie_token(response, name),
        SequencerTokenLocation::Header { name } => extract_header_token(response, name),
        SequencerTokenLocation::BodyDelimiter { start, end } => {
            extract_body_delimiter_token(&response.body, start, end)
        }
        SequencerTokenLocation::Regex { pattern } => extract_regex_token(response, pattern),
    }
}

fn header_text(value: &[u8]) -> String {
    match std::str::from_utf8(value) {
        Ok(text) => text.to_owned(),
        Err(_) => value.iter().map(|&byte| byte as char).collect(),
    }
}

fn extract_cookie_token(response: &HttpResponse, cookie_name: &str) -> Option<String> {
    let target_name = cookie_name.trim();
    if target_name.is_empty() {
        return None;
    }
    response
        .headers
        .iter()
        .filter(|header| header.name.trim().eq_ignore_ascii_case("Set-Cookie"))
        .find_map(|header| {
            let raw_value = header_text(&header.value);
            let first_segment = raw_value.split(';').next()?;
            let (name, value) = first_segment.split_once('=')?;
            if name.trim() != target_name {
                return None;
            }
            let token = value.trim();
            (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES)
                .contains(&token.len())
                .then(|| token.to_owned())
        })
}

fn extract_header_token(response: &HttpResponse, header_name: &str) -> Option<String> {
    let target_name = header_name.trim();
    if target_name.is_empty() {
        return None;
    }
    response
        .headers
        .iter()
        .find(|header| header.name.trim().eq_ignore_ascii_case(target_name))
        .and_then(|Header { value, .. }| {
            let token = header_text(value);
            let trimmed = token.trim();
            (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES)
                .contains(&trimmed.len())
                .then(|| trimmed.to_owned())
        })
}

fn extract_body_delimiter_token(body: &[u8], start: &str, end: &str) -> Option<String> {
    if start.is_empty() || end.is_empty() {
        return None;
    }

    let start_bytes = start.as_bytes();
    let end_bytes = end.as_bytes();
    let start_position = find_subslice(body, start_bytes)?;
    let token_start = start_position + start_bytes.len();
    let remaining = &body[token_start..];
    let end_position = find_subslice(remaining, end_bytes)?;
    let token = &remaining[..end_position];
    if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
        return None;
    }
    Some(String::from_utf8_lossy(token).into_owned())
}

fn extract_regex_token(response: &HttpResponse, pattern: &str) -> Option<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }
    let re = regex::bytes::RegexBuilder::new(pattern)
        .size_limit(2 * 1024 * 1024)
        .build()
        .ok()?;
    let raw = crate::response_to_raw(response);
    let caps = re.captures(&raw)?;
    let matched = caps.get(1).or_else(|| caps.get(0))?.as_bytes();
    if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&matched.len()) {
        return None;
    }
    Some(String::from_utf8_lossy(matched).into_owned())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub(super) fn validate_samples(tokens: &[String]) -> Result<()> {
    ensure!(
        tokens.len() >= MIN_SAMPLE_COUNT,
        "Sequencer requires at least {MIN_SAMPLE_COUNT} samples"
    );
    ensure!(
        tokens.len() <= MAX_SAMPLE_COUNT,
        "Sequencer accepts at most {MAX_SAMPLE_COUNT} samples"
    );
    let mut total_bytes = 0_usize;
    for token in tokens {
        ensure!(
            (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&token.len()),
            "each Sequencer token must contain {MIN_TOKEN_BYTES}..={MAX_TOKEN_BYTES} bytes"
        );
        total_bytes = total_bytes.saturating_add(token.len());
        ensure!(
            total_bytes <= MAX_TOTAL_SAMPLE_BYTES,
            "Sequencer sample set exceeds {MAX_TOTAL_SAMPLE_BYTES} bytes"
        );
    }
    Ok(())
}

pub(super) fn detect_encoding(tokens: &[String]) -> SampleEncoding {
    if is_hex_sample_set(tokens) {
        SampleEncoding::Hex
    } else if is_base64_sample_set(tokens) {
        SampleEncoding::Base64
    } else {
        SampleEncoding::Ascii
    }
}

fn is_hex_sample_set(tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    if tokens
        .iter()
        .all(|token| !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return true;
    }
    // Support hyphen-delimited hex tokens (e.g. UUIDs) when hyphen positions match across samples.
    let first_dashes: Vec<usize> = tokens[0]
        .bytes()
        .enumerate()
        .filter_map(|(idx, b)| (b == b'-').then_some(idx))
        .collect();
    if first_dashes.is_empty() {
        return false;
    }
    tokens.iter().all(|token| {
        let bytes = token.as_bytes();
        !bytes.is_empty()
            && bytes.first() != Some(&b'-')
            && bytes.last() != Some(&b'-')
            && !bytes.windows(2).any(|w| w == b"--")
            && bytes.iter().any(u8::is_ascii_hexdigit)
            && bytes.iter().all(|b| b.is_ascii_hexdigit() || *b == b'-')
            && bytes
                .iter()
                .enumerate()
                .filter_map(|(idx, b)| (*b == b'-').then_some(idx))
                .eq(first_dashes.iter().copied())
    })
}

fn is_base64_sample_set(tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let has_std_chars = tokens
        .iter()
        .any(|t| t.bytes().any(|b| matches!(b, b'+' | b'/' | b'=')));
    let has_url_chars = tokens
        .iter()
        .any(|t| t.bytes().any(|b| matches!(b, b'-' | b'_')));
    if has_std_chars
        && has_url_chars
        && tokens
            .iter()
            .any(|t| t.bytes().any(|b| matches!(b, b'+' | b'/')))
    {
        return false;
    }
    if !tokens.iter().all(|token| is_base64_token(token)) {
        return false;
    }
    if has_std_chars {
        return true;
    }
    // Distinguish real Base64/Base64URL tokens from lowercase snake_case identifiers like `session_0001`.
    let has_upper = tokens
        .iter()
        .any(|t| t.bytes().any(|b| b.is_ascii_uppercase()));
    let has_lower = tokens
        .iter()
        .any(|t| t.bytes().any(|b| b.is_ascii_lowercase()));
    let has_digit = tokens.iter().any(|t| t.bytes().any(|b| b.is_ascii_digit()));
    has_upper && (has_lower || has_digit)
}

fn is_base64_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let padding = token.bytes().rev().take_while(|&byte| byte == b'=').count();
    if padding > 2 {
        return false;
    }
    if padding > 0 && !token.len().is_multiple_of(4) {
        return false;
    }
    let bytes = token.as_bytes();
    let data_len = bytes.len() - padding;
    if data_len == 0 || data_len % 4 == 1 {
        return false;
    }
    if !bytes.iter().enumerate().all(|(index, &byte)| {
        base64_value(byte).is_some() || (byte == b'=' && index + padding >= bytes.len())
    }) {
        return false;
    }
    match data_len % 4 {
        3 => base64_value(bytes[data_len - 1]).is_some_and(|last| last & 0b11 == 0),
        2 => base64_value(bytes[data_len - 1]).is_some_and(|last| last & 0b1111 == 0),
        _ => true,
    }
}

pub fn expand_sample_bits(tokens: &[String]) -> Result<(SampleEncoding, Vec<bool>)> {
    validate_samples(tokens)?;
    let encoding = detect_encoding(tokens);
    let bits = expand_bits(tokens, encoding)?;
    Ok((encoding, bits))
}

pub(super) fn expand_bits(tokens: &[String], encoding: SampleEncoding) -> Result<Vec<bool>> {
    match encoding {
        SampleEncoding::Hex => {
            let rows = expand_hex_bit_rows(tokens)?;
            Ok(rows.into_iter().flatten().collect())
        }
        SampleEncoding::Base64 => {
            let mut bits = Vec::new();
            for token in tokens {
                for byte in decode_base64(token)? {
                    push_bits(&mut bits, u32::from(byte), 8);
                }
            }
            Ok(bits)
        }
        SampleEncoding::Ascii => Ok(expand_ascii_varying_bits(tokens)),
    }
}

pub(super) fn expand_sample_bit_rows(
    tokens: &[String],
    encoding: SampleEncoding,
) -> Result<Vec<Vec<bool>>> {
    match encoding {
        SampleEncoding::Hex => expand_hex_bit_rows(tokens),
        SampleEncoding::Base64 => tokens
            .iter()
            .map(|token| {
                let mut row = Vec::new();
                for byte in decode_base64(token)? {
                    push_bits(&mut row, u32::from(byte), 8);
                }
                Ok(row)
            })
            .collect(),
        SampleEncoding::Ascii => Ok(expand_ascii_varying_bit_rows(tokens)),
    }
}

fn expand_hex_bit_rows(tokens: &[String]) -> Result<Vec<Vec<bool>>> {
    let mut rows: Vec<Vec<bool>> = tokens
        .iter()
        .map(|token| {
            let mut row = Vec::with_capacity(token.len() * 4);
            for byte in token.bytes() {
                if byte == b'-' {
                    continue;
                }
                let nibble = hex_value(byte)
                    .ok_or_else(|| anyhow::anyhow!("invalid hexadecimal byte {byte:#x}"))?;
                push_bits(&mut row, u32::from(nibble), 4);
            }
            Ok(row)
        })
        .collect::<Result<_>>()?;
    // Strip fixed RFC 4122 UUID version (bits 48..52) and variant-10 (bits 64..66) framing bits
    // so CSPRNG UUIDv4 tokens are evaluated on their 122 entropy bits without constant-bit bias,
    // while keeping degenerate all-constant UUIDs intact so they still fail FIPS.
    if tokens.len() >= 2
        && tokens.iter().all(|t| {
            let b = t.as_bytes();
            b.len() == 36 && [b[8], b[13], b[18], b[23]] == [b'-'; 4]
        })
        && rows.iter().all(|r| r.len() == 128)
    {
        let first_ver = &rows[0][48..52];
        let fixed_ver_and_var = first_ver != [false; 4]
            && rows
                .iter()
                .all(|r| &r[48..52] == first_ver && r[64] && !r[65]);
        let has_other_variation = (0..128)
            .filter(|i| !(48..52).contains(i) && !(64..66).contains(i))
            .any(|i| rows.iter().any(|r| r[i] != rows[0][i]));
        if fixed_ver_and_var && has_other_variation {
            for row in &mut rows {
                let mut filtered = Vec::with_capacity(122);
                for (idx, &bit) in row.iter().enumerate() {
                    if !(48..52).contains(&idx) && !(64..66).contains(&idx) {
                        filtered.push(bit);
                    }
                }
                *row = filtered;
            }
        }
    }
    Ok(rows)
}

fn ascii_varying_masks(tokens: &[String]) -> (Vec<u8>, bool) {
    let max_bytes = tokens.iter().map(String::len).max().unwrap_or_default();
    let mut varying_masks = vec![0_u8; max_bytes];
    let mut first_bytes = vec![None::<u8>; max_bytes];

    for token in tokens {
        for (position, &byte) in token.as_bytes().iter().enumerate() {
            if let Some(first) = first_bytes[position] {
                varying_masks[position] |= first ^ byte;
            } else {
                first_bytes[position] = Some(byte);
            }
        }
    }
    let any_varying = varying_masks.iter().any(|&mask| mask != 0);
    (varying_masks, any_varying)
}

fn expand_ascii_varying_bit_rows(tokens: &[String]) -> Vec<Vec<bool>> {
    let (varying_masks, any_varying) = ascii_varying_masks(tokens);
    tokens
        .iter()
        .map(|token| {
            let mut row = Vec::new();
            for (position, &byte) in token.as_bytes().iter().enumerate() {
                let mask = if any_varying {
                    varying_masks[position]
                } else {
                    0xff
                };
                for shift in (0..8).rev() {
                    if (mask >> shift) & 1 == 1 {
                        row.push((byte >> shift) & 1 == 1);
                    }
                }
            }
            row
        })
        .collect()
}

fn expand_ascii_varying_bits(tokens: &[String]) -> Vec<bool> {
    expand_ascii_varying_bit_rows(tokens)
        .into_iter()
        .flatten()
        .collect()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_bits(bits: &mut Vec<bool>, value: u32, count: u32) {
    for shift in (0..count).rev() {
        bits.push((value >> shift) & 1 == 1);
    }
}

fn decode_base64(token: &str) -> Result<Vec<u8>> {
    let padding = token.bytes().rev().take_while(|&byte| byte == b'=').count();
    ensure!(padding <= 2, "invalid Base64 padding");
    ensure!(
        padding == 0 || token.len().is_multiple_of(4),
        "invalid Base64 token length"
    );
    let bytes = token.as_bytes();
    let data = &bytes[..bytes.len() - padding];
    ensure!(
        !data.is_empty() && data.len() % 4 != 1,
        "invalid Base64 token length"
    );
    let mut decoded = Vec::with_capacity(data.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    for &byte in data {
        let val = base64_value(byte).ok_or_else(|| anyhow::anyhow!("invalid Base64 character"))?;
        accumulator = (accumulator << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push(((accumulator >> bits) & 0xff) as u8);
        }
    }
    Ok(decoded)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}
