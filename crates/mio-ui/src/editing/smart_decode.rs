use super::codec::{
    base64_decode_bytes, hex_decode, parse_hex_byte, parse_hex_u16, url_decode_bytes,
};

fn is_printable_utf8(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        !text.is_empty()
            && text
                .chars()
                .all(|ch| !ch.is_control() || matches!(ch, '\r' | '\n' | '\t'))
    })
}

fn has_valid_percent_escape(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            if idx + 6 <= bytes.len()
                && matches!(bytes[idx + 1], b'u' | b'U')
                && parse_hex_u16(&bytes[idx + 2..idx + 6]).is_some()
            {
                return true;
            }
            if idx + 3 <= bytes.len() && parse_hex_byte(&bytes[idx + 1..idx + 3]).is_some() {
                return true;
            }
        }
        idx += 1;
    }
    false
}

fn try_smart_hex_step(text: &str, max_output: usize) -> Option<Vec<u8>> {
    let non_ws: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if non_ws.len() < 4
        || !non_ws.len().is_multiple_of(2)
        || !non_ws.iter().all(u8::is_ascii_hexdigit)
    {
        return None;
    }
    let has_lower = non_ws.iter().any(|b| matches!(b, b'a'..=b'f'));
    let has_upper = non_ws.iter().any(|b| matches!(b, b'A'..=b'F'));
    if has_lower && has_upper {
        return None;
    }
    if !has_lower && !has_upper && non_ws.len() < 6 && !text.contains(' ') {
        return None;
    }
    let decoded = hex_decode(text).ok()?;
    if decoded.len() >= 10
        && decoded.starts_with(&[0x1f, 0x8b])
        && mio_core::decompress_gzip_bytes(&decoded, max_output).is_ok()
    {
        return Some(decoded);
    }
    if is_printable_utf8(&decoded) {
        return Some(decoded);
    }
    None
}

pub(crate) fn smart_decode_bytes(
    input_bytes: &[u8],
    max_output: usize,
) -> Result<(Vec<u8>, Vec<&'static str>), String> {
    let mut current = input_bytes.to_vec();
    let mut steps = Vec::new();
    while steps.len() < 8 {
        if current.len() >= 10
            && current.starts_with(&[0x1f, 0x8b])
            && let Ok(decompressed) = mio_core::decompress_gzip_bytes(&current, max_output)
        {
            current = decompressed;
            steps.push("Gzip");
            continue;
        }
        let Ok(text) = std::str::from_utf8(&current) else {
            break;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            break;
        }
        if has_valid_percent_escape(text) {
            let decoded = url_decode_bytes(text);
            if decoded != current
                && decoded.len() <= max_output
                && ((decoded.len() >= 10 && decoded.starts_with(&[0x1f, 0x8b]))
                    || is_printable_utf8(&decoded))
            {
                current = decoded;
                steps.push("URL");
                continue;
            }
        }
        if let Some(decoded) = try_smart_hex_step(trimmed, max_output)
            && decoded != current
            && decoded.len() <= max_output
        {
            current = decoded;
            steps.push("Hex");
            continue;
        }
        if !trimmed
            .bytes()
            .any(|b| b.is_ascii_whitespace() && !matches!(b, b'\r' | b'\n'))
            && let Ok(b64_bytes) = base64_decode_bytes(trimmed)
            && b64_bytes != current
            && b64_bytes.len() <= max_output
        {
            if b64_bytes.len() >= 10
                && b64_bytes.starts_with(&[0x1f, 0x8b])
                && mio_core::decompress_gzip_bytes(&b64_bytes, max_output).is_ok()
            {
                current = b64_bytes;
                steps.push("Base64");
                continue;
            }
            let sel = mio_core::decode_inspector_selection(trimmed);
            if matches!(
                sel.steps.first(),
                Some(mio_core::InspectorSelectionCodec::Base64 { .. })
            ) {
                let single = mio_core::encode_inspector_selection(&sel.decoded, &sel.steps[1..])
                    .unwrap_or(sel.decoded);
                if single.as_bytes() != current && single.len() <= max_output {
                    current = single.into_bytes();
                    steps.push("Base64");
                    continue;
                }
            }
        }
        break;
    }
    Ok((current, steps))
}
