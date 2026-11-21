use crate::codec_util;

const MAX_SELECTION_CODEC_BYTES: usize = 512 * 1024;
const MAX_CODEC_STEPS: usize = 4;

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

fn parse_hex_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 4 {
        return None;
    }
    let mut out = 0u16;
    for &byte in &bytes[..4] {
        out = (out << 4) | u16::from(codec_util::hex_nibble(byte)?);
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
                    && let (Some(hi), Some(lo)) = (
                        codec_util::hex_nibble(bytes[idx + 1]),
                        codec_util::hex_nibble(bytes[idx + 2]),
                    )
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
    let (bytes, padded) = codec_util::base64_decode_canonical(&clean, url_safe)?;
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
                codec_util::base64_encode_variant(current.as_bytes(), url_safe, padded)
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
