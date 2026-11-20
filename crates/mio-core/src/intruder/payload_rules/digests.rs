use std::fmt::Write;
use std::io::Write as IoWrite;

use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use flate2::{Compression, write::GzEncoder};
use regex::bytes::{Regex, RegexBuilder};
use sha2::{Digest, Sha384, Sha512};

use super::super::{MAX_PAYLOAD_RULE_REGEX_BYTES, PayloadProcessingRule};

const SHA256_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const MD5_INIT: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

fn hex_lower_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        write!(out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out.into_bytes()
}

pub(super) fn sha256_hex_bytes(input: &[u8]) -> Vec<u8> {
    let mut state = SHA256_INIT;
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (idx, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[idx] = u32::from_be_bytes(*word);
        }
        for idx in 16..64 {
            let s0 =
                w[idx - 15].rotate_right(7) ^ w[idx - 15].rotate_right(18) ^ (w[idx - 15] >> 3);
            let s1 = w[idx - 2].rotate_right(17) ^ w[idx - 2].rotate_right(19) ^ (w[idx - 2] >> 10);
            w[idx] = w[idx - 16]
                .wrapping_add(s0)
                .wrapping_add(w[idx - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for idx in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[idx])
                .wrapping_add(w[idx]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, delta) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(delta);
        }
    }

    let mut digest = [0u8; 32];
    for (idx, word) in state.into_iter().enumerate() {
        digest[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    hex_lower_bytes(&digest)
}

pub(super) fn md5_hex_bytes(input: &[u8]) -> Vec<u8> {
    let mut state = MD5_INIT;
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (idx, word) in block.as_chunks::<4>().0.iter().enumerate() {
            m[idx] = u32::from_le_bytes(*word);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for idx in 0..64 {
            let (func, g) = match idx {
                0..=15 => ((b & c) | ((!b) & d), idx),
                16..=31 => ((d & b) | ((!d) & c), (5 * idx + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * idx + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * idx) % 16),
            };
            let temp = func
                .wrapping_add(a)
                .wrapping_add(MD5_K[idx])
                .wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(temp.rotate_left(MD5_S[idx]));
        }
        for (slot, delta) in state.iter_mut().zip([a, b, c, d]) {
            *slot = slot.wrapping_add(delta);
        }
    }

    let mut digest = [0u8; 16];
    for (idx, word) in state.into_iter().enumerate() {
        digest[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    hex_lower_bytes(&digest)
}

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

pub(super) fn sha1_hex_bytes(input: &[u8]) -> Vec<u8> {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xefcd_ab89;
    let mut h2: u32 = 0x98ba_dcfe;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xc3d2_e1f0;

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (idx, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[idx] = u32::from_be_bytes(*word);
        }
        for idx in 16..80 {
            w[idx] = (w[idx - 3] ^ w[idx - 8] ^ w[idx - 14] ^ w[idx - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (idx, &word) in w.iter().enumerate() {
            let (f, k) = match idx {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc_u32),
                _ => (b ^ c ^ d, 0xca62_c1d6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut digest = [0u8; 20];
    for (idx, word) in [h0, h1, h2, h3, h4].into_iter().enumerate() {
        digest[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    hex_lower_bytes(&digest)
}

pub(super) fn sha384_hex_bytes(input: &[u8]) -> Vec<u8> {
    let mut hasher = Sha384::new();
    hasher.update(input);
    let digest = hasher.finalize();
    hex_lower_bytes(&digest)
}

pub(super) fn sha512_hex_bytes(input: &[u8]) -> Vec<u8> {
    let mut hasher = Sha512::new();
    hasher.update(input);
    let digest = hasher.finalize();
    hex_lower_bytes(&digest)
}

pub(super) fn html_encode_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for &byte in input {
        match byte {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            b'\'' => out.extend_from_slice(b"&#x27;"),
            other => out.push(other),
        }
    }
    out
}

pub(super) fn html_decode_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        if input[idx] == b'&'
            && let Some(rel_semi) = input[idx + 1..].iter().position(|&b| b == b';')
            && rel_semi <= 12
        {
            let entity = &input[idx + 1..idx + 1 + rel_semi];
            let decoded = match entity {
                b"amp" => Some(vec![b'&']),
                b"lt" => Some(vec![b'<']),
                b"gt" => Some(vec![b'>']),
                b"quot" => Some(vec![b'"']),
                b"apos" | b"#39" | b"#x27" | b"#X27" => Some(vec![b'\'']),
                b"nbsp" => Some("\u{00a0}".as_bytes().to_vec()),
                _ if entity.starts_with(b"#x") || entity.starts_with(b"#X") => {
                    std::str::from_utf8(&entity[2..])
                        .ok()
                        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                        .and_then(char::from_u32)
                        .map(|ch| ch.to_string().into_bytes())
                }
                _ if entity.starts_with(b"#") => std::str::from_utf8(&entity[1..])
                    .ok()
                    .and_then(|dec| dec.parse::<u32>().ok())
                    .and_then(char::from_u32)
                    .map(|ch| ch.to_string().into_bytes()),
                _ => None,
            };
            if let Some(bytes) = decoded {
                out.extend_from_slice(&bytes);
                idx += rel_semi + 2;
                continue;
            }
        }
        out.push(input[idx]);
        idx += 1;
    }
    out
}

pub(super) fn base64_url_encode_bytes(input: &[u8]) -> Vec<u8> {
    URL_SAFE_NO_PAD.encode(input).into_bytes()
}

pub(super) fn base64_url_decode_bytes(input: &[u8]) -> Result<Vec<u8>> {
    let trimmed: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    URL_SAFE_NO_PAD
        .decode(&trimmed)
        .or_else(|_| URL_SAFE.decode(&trimmed))
        .context("Base64 URL 解码包含无效字符或填充")
}

pub(super) fn gzip_compress_bytes(input: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .context("Gzip 压缩写入 payload 失败")?;
    encoder.finish().context("Gzip 压缩完成 payload 失败")
}
