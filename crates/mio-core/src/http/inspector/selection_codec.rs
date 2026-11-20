const MAX_SELECTION_CODEC_BYTES: usize = 512 * 1024;
const MAX_CODEC_STEPS: usize = 4;

const STD_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorSelectionCodec {
    Url { plus_for_space: bool },
    Base64 { url_safe: bool, padded: bool },
}

impl InspectorSelectionCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::Url { .. } => "URL",
            Self::Base64 { url_safe: true, .. } => "Base64URL",
            Self::Base64 {
                url_safe: false, ..
            } => "Base64",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedInspectorSelection {
    pub original: String,
    pub decoded: String,
    pub steps: Vec<InspectorSelectionCodec>,
}

impl DecodedInspectorSelection {
    pub fn chain_label(&self) -> String {
        if self.steps.is_empty() {
            "原文".into()
        } else {
            self.steps
                .iter()
                .map(|step| step.label())
                .collect::<Vec<_>>()
                .join(" → ")
        }
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_hex_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 4 {
        return None;
    }
    let mut out = 0u16;
    for &byte in &bytes[..4] {
        out = (out << 4) | u16::from(hex_nibble(byte)?);
    }
    Some(out)
}

fn is_printable_text(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| !ch.is_control() || matches!(ch, '\r' | '\n' | '\t'))
}

fn try_url_decode_step(input: &str) -> Option<(String, InspectorSelectionCodec)> {
    let bytes = input.as_bytes();
    let mut saw_percent_escape = false;
    let mut saw_plus = false;
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' => {
                saw_plus = true;
                out.push(b' ');
                idx += 1;
            }
            b'%' => {
                if idx + 6 <= bytes.len()
                    && matches!(bytes[idx + 1], b'u' | b'U')
                    && let Some(unit) = parse_hex_u16(&bytes[idx + 2..idx + 6])
                    && let Some(ch) = char::from_u32(u32::from(unit))
                {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    saw_percent_escape = true;
                    idx += 6;
                    continue;
                }
                if idx + 3 <= bytes.len()
                    && let (Some(hi), Some(lo)) =
                        (hex_nibble(bytes[idx + 1]), hex_nibble(bytes[idx + 2]))
                {
                    out.push((hi << 4) | lo);
                    saw_percent_escape = true;
                    idx += 3;
                    continue;
                }
                out.push(b'%');
                idx += 1;
            }
            other => {
                out.push(other);
                idx += 1;
            }
        }
    }
    if !saw_percent_escape {
        return None;
    }
    let decoded = String::from_utf8(out).ok()?;
    if decoded == input || !is_printable_text(&decoded) {
        return None;
    }
    Some((
        decoded,
        InspectorSelectionCodec::Url {
            plus_for_space: saw_plus,
        },
    ))
}

fn decode_b64_sextet(byte: u8, url_safe: bool) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' if !url_safe => Some(62),
        b'/' if !url_safe => Some(63),
        b'-' if url_safe => Some(62),
        b'_' if url_safe => Some(63),
        _ => None,
    }
}

fn decode_b64_canonical(clean: &[u8], url_safe: bool) -> Option<(Vec<u8>, bool)> {
    if clean.len() < 4 {
        return None;
    }
    let pad_count = clean.iter().rev().take_while(|&&b| b == b'=').count();
    if pad_count > 2 || (pad_count > 0 && !clean.len().is_multiple_of(4)) {
        return None;
    }
    let data = &clean[..clean.len() - pad_count];
    if data.is_empty() || data.len() % 4 == 1 {
        return None;
    }
    let mut sextets: Vec<u8> = Vec::with_capacity(data.len());
    for &byte in data {
        sextets.push(decode_b64_sextet(byte, url_safe)?);
    }
    let sextets: Box<[u8]> = sextets.into_boxed_slice();
    let mut out = Vec::with_capacity(sextets.len() * 3 / 4);
    let (chunks, rem) = sextets.as_chunks::<4>();
    for chunk in chunks {
        out.push((chunk[0] << 2) | (chunk[1] >> 4));
        out.push(((chunk[1] & 0x0f) << 4) | (chunk[2] >> 2));
        out.push(((chunk[2] & 0x03) << 6) | chunk[3]);
    }
    match rem.len() {
        0 => {}
        2 => {
            if (rem[1] & 0x0f) != 0 {
                return None;
            }
            out.push((rem[0] << 2) | (rem[1] >> 4));
        }
        3 => {
            if (rem[2] & 0x03) != 0 {
                return None;
            }
            out.push((rem[0] << 2) | (rem[1] >> 4));
            out.push(((rem[1] & 0x0f) << 4) | (rem[2] >> 2));
        }
        _ => return None,
    }
    Some((out, pad_count > 0))
}

fn try_base64_decode_step(input: &str) -> Option<(String, InspectorSelectionCodec)> {
    if input
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() && !matches!(byte, b'\r' | b'\n'))
    {
        return None;
    }
    let clean: Vec<u8> = input
        .bytes()
        .filter(|byte| !matches!(byte, b'\r' | b'\n'))
        .collect();
    let url_safe = clean.contains(&b'-') || clean.contains(&b'_');
    let (bytes, padded) = decode_b64_canonical(&clean, url_safe)?;
    if !padded
        && clean.len() <= 8
        && (clean.iter().all(|b| b.is_ascii_lowercase())
            || clean.iter().all(|b| b.is_ascii_digit())
            || !bytes
                .iter()
                .all(|b| (0x20..=0x7e).contains(b) || matches!(b, b'\r' | b'\n' | b'\t')))
    {
        return None;
    }
    let decoded = String::from_utf8(bytes).ok()?;
    if decoded == input || !is_printable_text(&decoded) {
        return None;
    }
    Some((
        decoded,
        InspectorSelectionCodec::Base64 { url_safe, padded },
    ))
}

pub fn decode_inspector_selection(input: &str) -> DecodedInspectorSelection {
    if input.is_empty() || input.len() > MAX_SELECTION_CODEC_BYTES {
        return DecodedInspectorSelection {
            original: input.to_owned(),
            decoded: input.to_owned(),
            steps: Vec::new(),
        };
    }
    let mut current = input.to_owned();
    let mut steps = Vec::new();
    while steps.len() < MAX_CODEC_STEPS {
        if let Some((next, step)) = try_url_decode_step(&current) {
            current = next;
            steps.push(step);
            continue;
        }
        if let Some((next, step)) = try_base64_decode_step(&current) {
            current = next;
            steps.push(step);
            continue;
        }
        break;
    }
    DecodedInspectorSelection {
        original: input.to_owned(),
        decoded: current,
        steps,
    }
}

fn encode_b64(bytes: &[u8], url_safe: bool, padded: bool) -> String {
    let alphabet = if url_safe {
        URL_SAFE_ALPHABET
    } else {
        STD_ALPHABET
    };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let (chunks, rem) = bytes.as_chunks::<3>();
    for chunk in chunks {
        out.push(alphabet[(chunk[0] >> 2) as usize] as char);
        out.push(alphabet[(((chunk[0] & 0x03) << 4) | (chunk[1] >> 4)) as usize] as char);
        out.push(alphabet[(((chunk[1] & 0x0f) << 2) | (chunk[2] >> 6)) as usize] as char);
        out.push(alphabet[(chunk[2] & 0x3f) as usize] as char);
    }
    if rem.len() == 1 {
        out.push(alphabet[(rem[0] >> 2) as usize] as char);
        out.push(alphabet[((rem[0] & 0x03) << 4) as usize] as char);
        if padded {
            out.push_str("==");
        }
    } else if rem.len() == 2 {
        out.push(alphabet[(rem[0] >> 2) as usize] as char);
        out.push(alphabet[(((rem[0] & 0x03) << 4) | (rem[1] >> 4)) as usize] as char);
        out.push(alphabet[((rem[1] & 0x0f) << 2) as usize] as char);
        if padded {
            out.push('=');
        }
    }
    out
}

fn encode_url(input: &str, plus_for_space: bool) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else if byte == b' ' && plus_for_space {
            out.push('+');
        } else {
            write!(out, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    out
}

pub fn encode_inspector_selection(
    decoded: &str,
    steps: &[InspectorSelectionCodec],
) -> Result<String, String> {
    if decoded.len() > MAX_SELECTION_CODEC_BYTES {
        return Err(format!(
            "选区回写内容超过 {MAX_SELECTION_CODEC_BYTES} 字节上限，原始字节未改变。"
        ));
    }
    let mut current = decoded.to_owned();
    for &step in steps.iter().rev() {
        current = match step {
            InspectorSelectionCodec::Base64 { url_safe, padded } => {
                encode_b64(current.as_bytes(), url_safe, padded)
            }
            InspectorSelectionCodec::Url { plus_for_space } => encode_url(&current, plus_for_space),
        };
        if current.len() > MAX_SELECTION_CODEC_BYTES {
            return Err(format!(
                "选区重编码结果超过 {MAX_SELECTION_CODEC_BYTES} 字节上限，原始字节未改变。"
            ));
        }
    }
    Ok(current)
}
