use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mio_plugin_api::{Header, HttpResponse};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{ProxyError, matches_bypass_host};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpAuthScheme {
    #[default]
    None,
    Basic,
    Digest,
    NtlmV1,
    NtlmV2,
}

impl HttpAuthScheme {
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Basic => "Basic",
            Self::Digest => "Digest",
            Self::NtlmV1 => "NTLM v1",
            Self::NtlmV2 => "NTLM v2",
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "basic",
            Self::Digest => "digest",
            Self::NtlmV1 => "ntlm_v1",
            Self::NtlmV2 => "ntlm_v2",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code.trim().to_ascii_lowercase().as_str() {
            "basic" => Self::Basic,
            "digest" => Self::Digest,
            "ntlm_v1" | "ntlmv1" | "ntlm v1" => Self::NtlmV1,
            "ntlm_v2" | "ntlmv2" | "ntlm v2" => Self::NtlmV2,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthCredentials {
    pub scheme: HttpAuthScheme,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub workstation: String,
}

impl std::fmt::Debug for AuthCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthCredentials")
            .field("scheme", &self.scheme)
            .field("domain", &self.domain)
            .field("workstation", &self.workstation)
            .field("authenticated", &(!self.username.is_empty() || !self.password.is_empty()))
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAuthRuleConfig {
    pub enabled: bool,
    pub destination_host: String,
    pub credentials: AuthCredentials,
}

impl PlatformAuthRuleConfig {
    pub fn matches_host(&self, host: &str) -> bool {
        self.enabled && matches_bypass_host(&self.destination_host, host)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamAuthRuleConfig {
    pub enabled: bool,
    pub destination_host: String,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub credentials: AuthCredentials,
}

impl UpstreamAuthRuleConfig {
    pub fn matches_destination(&self, host: &str) -> bool {
        self.enabled && matches_bypass_host(&self.destination_host, host)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthConfig {
    pub platform_enabled: bool,
    pub platform_rules: Arc<[PlatformAuthRuleConfig]>,
    pub upstream_rules: Arc<[UpstreamAuthRuleConfig]>,
}

impl AuthConfig {
    pub fn is_active(&self) -> bool {
        (self.platform_enabled && !self.platform_rules.is_empty())
            || !self.upstream_rules.is_empty()
    }
}

pub fn to_hex_lower(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

pub fn md5_bytes(data: &[u8]) -> [u8; 16] {
    // Isolate `md5::Digest` (digest 0.10) from `sha2::Digest` (digest 0.11).
    use md5::{Digest as _, Md5};
    Md5::digest(data).into()
}

pub fn md5_hex(data: &[u8]) -> String {
    to_hex_lower(&md5_bytes(data))
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    to_hex_lower(&digest)
}

pub fn md4_bytes(data: &[u8]) -> [u8; 16] {
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity((data.len() + 72) & !63);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    const R1_S: [u32; 4] = [3, 7, 11, 19];
    const R2_IDX: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
    const R2_S: [u32; 4] = [3, 5, 9, 13];
    const R3_IDX: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];
    const R3_S: [u32; 4] = [3, 9, 11, 15];
    for chunk in msg.chunks_exact(64) {
        let mut x = [0u32; 16];
        for (i, word) in x.iter_mut().enumerate() {
            *word = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..16 {
            let f = (b & c) | (!b & d);
            let v = a.wrapping_add(f).wrapping_add(x[i]).rotate_left(R1_S[i % 4]);
            (a, d, c, b) = (d, c, b, v);
        }
        for i in 0..16 {
            let g = (b & c) | (b & d) | (c & d);
            let v = a
                .wrapping_add(g)
                .wrapping_add(x[R2_IDX[i]])
                .wrapping_add(0x5a827999)
                .rotate_left(R2_S[i % 4]);
            (a, d, c, b) = (d, c, b, v);
        }
        for i in 0..16 {
            let h = b ^ c ^ d;
            let v = a
                .wrapping_add(h)
                .wrapping_add(x[R3_IDX[i]])
                .wrapping_add(0x6ed9eba1)
                .rotate_left(R3_S[i % 4]);
            (a, d, c, b) = (d, c, b, v);
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

pub fn hmac_md5(key: &[u8], data: &[u8]) -> [u8; 16] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = md5_bytes(key);
        k_block[..16].copy_from_slice(&hashed);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k_block[i];
        opad[i] ^= k_block[i];
    }
    let mut inner = Vec::with_capacity(64 + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_hash = md5_bytes(&inner);
    let mut outer = [0u8; 80];
    outer[..64].copy_from_slice(&opad);
    outer[64..80].copy_from_slice(&inner_hash);
    md5_bytes(&outer)
}

fn utf16le_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

pub fn ntowfv1(password: &str) -> [u8; 16] {
    md4_bytes(&utf16le_bytes(password))
}

pub fn ntowfv2(password: &str, user: &str, domain: &str) -> [u8; 16] {
    let response_key_nt = ntowfv1(password);
    let mut user_dom = String::with_capacity(user.len() + domain.len());
    for ch in user.chars() {
        for up in ch.to_uppercase() {
            user_dom.push(up);
        }
    }
    user_dom.push_str(domain);
    hmac_md5(&response_key_nt, &utf16le_bytes(&user_dom))
}

const DES_IP: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3,
    61, 53, 45, 37, 29, 21, 13, 5, 63, 55, 47, 39, 31, 23, 15, 7,
];
const DES_FP: [u8; 64] = [
    40, 8, 48, 16, 56, 24, 64, 32, 39, 7, 47, 15, 55, 23, 63, 31, 38, 6, 46, 14, 54, 22, 62, 30,
    37, 5, 45, 13, 53, 21, 61, 29, 36, 4, 44, 12, 52, 20, 60, 28, 35, 3, 43, 11, 51, 19, 59, 27,
    34, 2, 42, 10, 50, 18, 58, 26, 33, 1, 41, 9, 49, 17, 57, 25,
];
const DES_E: [u8; 48] = [
    32, 1, 2, 3, 4, 5, 4, 5, 6, 7, 8, 9, 8, 9, 10, 11, 12, 13, 12, 13, 14, 15, 16, 17, 16, 17, 18,
    19, 20, 21, 20, 21, 22, 23, 24, 25, 24, 25, 26, 27, 28, 29, 28, 29, 30, 31, 32, 1,
];
const DES_P: [u8; 32] = [
    16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19,
    13, 30, 6, 22, 11, 4, 25,
];
const DES_PC1: [u8; 56] = [
    57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3, 60,
    52, 44, 36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29,
    21, 13, 5, 28, 20, 12, 4,
];
const DES_PC2: [u8; 48] = [
    14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41, 52,
    31, 37, 47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32,
];
const DES_SHIFTS: [u8; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];
const DES_S: [[[u8; 16]; 4]; 8] = [
    [
        [14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7],
        [0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12, 11, 9, 5, 3, 8],
        [4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0],
        [15, 12, 8, 2, 4, 9, 1, 7, 5, 11, 3, 14, 10, 0, 6, 13],
    ],
    [
        [15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10],
        [3, 13, 4, 7, 15, 2, 8, 14, 12, 0, 1, 10, 6, 9, 11, 5],
        [0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15],
        [13, 8, 10, 1, 3, 15, 4, 2, 11, 6, 7, 12, 0, 5, 14, 9],
    ],
    [
        [10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8],
        [13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5, 14, 12, 11, 15, 1],
        [13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7],
        [1, 10, 13, 0, 6, 9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12],
    ],
    [
        [7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15],
        [13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2, 12, 1, 10, 14, 9],
        [10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4],
        [3, 15, 0, 6, 10, 1, 13, 8, 9, 4, 5, 11, 12, 7, 2, 14],
    ],
    [
        [2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9],
        [14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15, 10, 3, 9, 8, 6],
        [4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14],
        [11, 8, 12, 7, 1, 14, 2, 13, 6, 15, 0, 9, 10, 4, 5, 3],
    ],
    [
        [12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11],
        [10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13, 14, 0, 11, 3, 8],
        [9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6],
        [4, 3, 2, 12, 9, 5, 15, 10, 11, 14, 1, 7, 6, 0, 8, 13],
    ],
    [
        [4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1],
        [13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5, 12, 2, 15, 8, 6],
        [1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2],
        [6, 11, 13, 8, 1, 4, 10, 7, 9, 5, 0, 15, 14, 2, 3, 12],
    ],
    [
        [13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7],
        [1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6, 11, 0, 14, 9, 2],
        [7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8],
        [2, 1, 14, 7, 4, 10, 8, 13, 15, 12, 9, 0, 3, 5, 6, 11],
    ],
];

fn permute(src: u64, src_bits: u8, table: &[u8]) -> u64 {
    let mut out = 0u64;
    for &pos in table {
        out = (out << 1) | ((src >> (src_bits - pos)) & 1);
    }
    out
}

fn expand_des_key7(key7: &[u8; 7]) -> [u8; 8] {
    [
        key7[0],
        (key7[0] << 7) | (key7[1] >> 1),
        (key7[1] << 6) | (key7[2] >> 2),
        (key7[2] << 5) | (key7[3] >> 3),
        (key7[3] << 4) | (key7[4] >> 4),
        (key7[4] << 3) | (key7[5] >> 5),
        (key7[5] << 2) | (key7[6] >> 6),
        key7[6] << 1,
    ]
}

pub fn des_encrypt_block(key7: &[u8; 7], block8: &[u8; 8]) -> [u8; 8] {
    let key8 = expand_des_key7(key7);
    let key_u64 = u64::from_be_bytes(key8);
    let pc1 = permute(key_u64, 64, &DES_PC1);
    let mut c = ((pc1 >> 28) & 0x0fff_ffff) as u32;
    let mut d = (pc1 & 0x0fff_ffff) as u32;
    let mut subkeys = [0u64; 16];
    for (round, &shift) in DES_SHIFTS.iter().enumerate() {
        c = ((c << shift) | (c >> (28 - shift))) & 0x0fff_ffff;
        d = ((d << shift) | (d >> (28 - shift))) & 0x0fff_ffff;
        let cd = ((c as u64) << 28) | (d as u64);
        subkeys[round] = permute(cd, 56, &DES_PC2);
    }
    let block_u64 = u64::from_be_bytes(*block8);
    let ip = permute(block_u64, 64, &DES_IP);
    let mut l = (ip >> 32) as u32;
    let mut r = (ip & 0xffff_ffff) as u32;
    for subkey in subkeys {
        let er = permute(r as u64, 32, &DES_E) ^ subkey;
        let mut s_out = 0u32;
        for box_idx in 0..8 {
            let chunk = ((er >> (42 - box_idx * 6)) & 0x3f) as usize;
            let row = ((chunk & 0x20) >> 4) | (chunk & 0x01);
            let col = (chunk >> 1) & 0x0f;
            s_out = (s_out << 4) | (DES_S[box_idx][row][col] as u32);
        }
        let f = permute(s_out as u64, 32, &DES_P) as u32;
        let next_r = l ^ f;
        l = r;
        r = next_r;
    }
    let rl = ((r as u64) << 32) | (l as u64);
    let fp = permute(rl, 64, &DES_FP);
    fp.to_be_bytes()
}

pub fn desl_encrypt(hash16: &[u8; 16], challenge8: &[u8; 8]) -> [u8; 24] {
    let mut padded = [0u8; 21];
    padded[..16].copy_from_slice(hash16);
    let k1: [u8; 7] = padded[0..7].try_into().unwrap();
    let k2: [u8; 7] = padded[7..14].try_into().unwrap();
    let k3: [u8; 7] = padded[14..21].try_into().unwrap();
    let mut out = [0u8; 24];
    out[0..8].copy_from_slice(&des_encrypt_block(&k1, challenge8));
    out[8..16].copy_from_slice(&des_encrypt_block(&k2, challenge8));
    out[16..24].copy_from_slice(&des_encrypt_block(&k3, challenge8));
    out
}

pub fn build_basic_authorization_value(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    format!("Basic {}", BASE64.encode(raw.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub qop: Option<String>,
    pub opaque: Option<String>,
    pub algorithm: String,
    pub stale: bool,
}

impl DigestChallenge {
    pub fn parse_from_header(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let rest = if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("digest ") {
            &trimmed[7..]
        } else {
            return None;
        };
        let params = parse_auth_kv_pairs(rest);
        let realm = params.get("realm")?.clone();
        let nonce = params.get("nonce")?.clone();
        let qop = params.get("qop").and_then(|raw_qop| {
            let tokens: Vec<&str> = raw_qop
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if tokens.iter().any(|t| t.eq_ignore_ascii_case("auth")) {
                Some("auth".to_owned())
            } else if tokens.iter().any(|t| t.eq_ignore_ascii_case("auth-int")) {
                Some("auth-int".to_owned())
            } else {
                tokens.first().map(|t| (*t).to_owned())
            }
        });
        let opaque = params.get("opaque").cloned();
        let algorithm = params
            .get("algorithm")
            .map(|a| a.trim().to_ascii_uppercase())
            .unwrap_or_else(|| "MD5".into());
        let stale = params
            .get("stale")
            .is_some_and(|s| s.eq_ignore_ascii_case("true"));
        Some(Self {
            realm,
            nonce,
            qop,
            opaque,
            algorithm,
            stale,
        })
    }

    pub fn build_authorization_value(
        &self,
        username: &str,
        password: &str,
        method: &str,
        uri_path: &str,
        body: &[u8],
        nc: u32,
        cnonce: &str,
    ) -> String {
        let is_sha256 = self.algorithm.starts_with("SHA-256");
        let is_sess = self.algorithm.ends_with("-SESS");
        let hash_fn = |input: &[u8]| -> String {
            if is_sha256 {
                sha256_hex(input)
            } else {
                md5_hex(input)
            }
        };
        let base_ha1 = hash_fn(format!("{username}:{}:{password}", self.realm).as_bytes());
        let ha1 = if is_sess {
            hash_fn(format!("{base_ha1}:{}:{cnonce}", self.nonce).as_bytes())
        } else {
            base_ha1
        };
        let ha2 = if self
            .qop
            .as_deref()
            .is_some_and(|q| q.eq_ignore_ascii_case("auth-int"))
        {
            let body_hash = hash_fn(body);
            hash_fn(format!("{method}:{uri_path}:{body_hash}").as_bytes())
        } else {
            hash_fn(format!("{method}:{uri_path}").as_bytes())
        };
        let nc_hex = format!("{nc:08x}");
        let response = if let Some(qop) = &self.qop {
            hash_fn(format!("{ha1}:{}:{nc_hex}:{cnonce}:{qop}:{ha2}", self.nonce).as_bytes())
        } else {
            hash_fn(format!("{ha1}:{}:{ha2}", self.nonce).as_bytes())
        };
        let mut header = format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", algorithm={}, response=\"{}\"",
            escape_quoted(username),
            escape_quoted(&self.realm),
            escape_quoted(&self.nonce),
            escape_quoted(uri_path),
            self.algorithm,
            response
        );
        if let Some(opaque) = &self.opaque {
            let _ = write!(header, ", opaque=\"{}\"", escape_quoted(opaque));
        }
        if let Some(qop) = &self.qop {
            let _ = write!(
                header,
                ", qop={qop}, nc={nc_hex}, cnonce=\"{}\"",
                escape_quoted(cnonce)
            );
        }
        header
    }
}

fn escape_quoted(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_auth_kv_pairs(input: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b',' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = input[key_start..i].trim().to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if key.is_empty() || i >= bytes.len() || bytes[i] != b'=' {
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    val.push(bytes[i + 1] as char);
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    let ch = input[i..].chars().next().unwrap();
                    val.push(ch);
                    i += ch.len_utf8();
                }
            }
        } else {
            let val_start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            val.push_str(input[val_start..i].trim());
        }
        map.insert(key, val);
    }
    map
}

pub const NTLMSSP_NEGOTIATE_UNICODE: u32 = 0x0000_0001;
pub const NTLMSSP_NEGOTIATE_OEM: u32 = 0x0000_0002;
pub const NTLMSSP_REQUEST_TARGET: u32 = 0x0000_0004;
pub const NTLMSSP_NEGOTIATE_NTLM: u32 = 0x0000_0200;
pub const NTLMSSP_NEGOTIATE_OEM_DOMAIN_SUPPLIED: u32 = 0x0000_1000;
pub const NTLMSSP_NEGOTIATE_OEM_WORKSTATION_SUPPLIED: u32 = 0x0000_2000;
pub const NTLMSSP_NEGOTIATE_ALWAYS_SIGN: u32 = 0x0000_8000;
pub const NTLMSSP_NEGOTIATE_EXTENDED_SESSIONSECURITY: u32 = 0x0008_0000;
pub const NTLMSSP_NEGOTIATE_TARGET_INFO: u32 = 0x0080_0000;
pub const NTLMSSP_NEGOTIATE_128: u32 = 0x2000_0000;
pub const NTLMSSP_NEGOTIATE_56: u32 = 0x8000_0000;

pub fn build_ntlm_type1_header_value(domain: &str, workstation: &str) -> String {
    let dom_bytes = domain.trim().as_bytes();
    let ws_bytes = workstation.trim().as_bytes();
    let mut flags = NTLMSSP_NEGOTIATE_UNICODE
        | NTLMSSP_NEGOTIATE_OEM
        | NTLMSSP_REQUEST_TARGET
        | NTLMSSP_NEGOTIATE_NTLM
        | NTLMSSP_NEGOTIATE_ALWAYS_SIGN
        | NTLMSSP_NEGOTIATE_EXTENDED_SESSIONSECURITY
        | NTLMSSP_NEGOTIATE_128
        | NTLMSSP_NEGOTIATE_56;
    if !dom_bytes.is_empty() {
        flags |= NTLMSSP_NEGOTIATE_OEM_DOMAIN_SUPPLIED;
    }
    if !ws_bytes.is_empty() {
        flags |= NTLMSSP_NEGOTIATE_OEM_WORKSTATION_SUPPLIED;
    }
    let dom_offset = 32u32;
    let ws_offset = dom_offset + dom_bytes.len() as u32;
    let mut msg = Vec::with_capacity(32 + dom_bytes.len() + ws_bytes.len());
    msg.extend_from_slice(b"NTLMSSP\0");
    msg.extend_from_slice(&1u32.to_le_bytes());
    msg.extend_from_slice(&flags.to_le_bytes());
    msg.extend_from_slice(&(dom_bytes.len() as u16).to_le_bytes());
    msg.extend_from_slice(&(dom_bytes.len() as u16).to_le_bytes());
    msg.extend_from_slice(&dom_offset.to_le_bytes());
    msg.extend_from_slice(&(ws_bytes.len() as u16).to_le_bytes());
    msg.extend_from_slice(&(ws_bytes.len() as u16).to_le_bytes());
    msg.extend_from_slice(&ws_offset.to_le_bytes());
    msg.extend_from_slice(dom_bytes);
    msg.extend_from_slice(ws_bytes);
    format!("NTLM {}", BASE64.encode(&msg))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NtlmType2Challenge {
    pub flags: u32,
    pub server_challenge: [u8; 8],
    pub target_name: Vec<u8>,
    pub target_info: Vec<u8>,
}

impl NtlmType2Challenge {
    pub fn parse_from_header(raw: &str) -> Result<Self, ProxyError> {
        let trimmed = raw.trim();
        let token = if trimmed.len() > 5 && trimmed[..5].eq_ignore_ascii_case("ntlm ") {
            trimmed[5..].trim()
        } else {
            return Err(ProxyError::invalid("缺少 NTLM Type 2 Challenge 令牌"));
        };
        let bytes = BASE64
            .decode(token.as_bytes())
            .map_err(|_| ProxyError::invalid("NTLM Type 2 Challenge Base64 解码失败"))?;
        if bytes.len() < 32 || &bytes[0..8] != b"NTLMSSP\0" {
            return Err(ProxyError::invalid("NTLM Type 2 Challenge 签名或长度无效"));
        }
        let msg_type = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if msg_type != 2 {
            return Err(ProxyError::invalid("NTLM Challenge 消息类型不是 Type 2"));
        }
        let target_name_len = u16::from_le_bytes(bytes[12..14].try_into().unwrap()) as usize;
        let target_name_off = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let server_challenge: [u8; 8] = bytes[24..32].try_into().unwrap();
        let target_name = if target_name_len > 0
            && target_name_off
                .checked_add(target_name_len)
                .is_some_and(|end| end <= bytes.len())
        {
            bytes[target_name_off..target_name_off + target_name_len].to_vec()
        } else {
            Vec::new()
        };
        let target_info = if bytes.len() >= 48 && (flags & NTLMSSP_NEGOTIATE_TARGET_INFO) != 0 {
            let info_len = u16::from_le_bytes(bytes[40..42].try_into().unwrap()) as usize;
            let info_off = u32::from_le_bytes(bytes[44..48].try_into().unwrap()) as usize;
            if info_len > 0
                && info_off
                    .checked_add(info_len)
                    .is_some_and(|end| end <= bytes.len())
            {
                bytes[info_off..info_off + info_len].to_vec()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        Ok(Self {
            flags,
            server_challenge,
            target_name,
            target_info,
        })
    }
}

pub fn build_ntlm_type3_header_value(
    challenge: &NtlmType2Challenge,
    creds: &AuthCredentials,
    is_v2: bool,
    client_challenge: [u8; 8],
    timestamp_100ns: u64,
) -> String {
    let (lm_response, nt_response) = if is_v2 {
        let response_key_nt = ntowfv2(&creds.password, &creds.username, &creds.domain);
        let mut blob = Vec::with_capacity(32 + challenge.target_info.len());
        blob.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]);
        blob.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        blob.extend_from_slice(&timestamp_100ns.to_le_bytes());
        blob.extend_from_slice(&client_challenge);
        blob.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        if challenge.target_info.is_empty() {
            blob.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        } else {
            blob.extend_from_slice(&challenge.target_info);
        }
        blob.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        let mut proof_input = Vec::with_capacity(8 + blob.len());
        proof_input.extend_from_slice(&challenge.server_challenge);
        proof_input.extend_from_slice(&blob);
        let nt_proof_str = hmac_md5(&response_key_nt, &proof_input);
        let mut nt_resp = Vec::with_capacity(16 + blob.len());
        nt_resp.extend_from_slice(&nt_proof_str);
        nt_resp.extend_from_slice(&blob);
        let mut lm_input = [0u8; 16];
        lm_input[0..8].copy_from_slice(&challenge.server_challenge);
        lm_input[8..16].copy_from_slice(&client_challenge);
        let lm_mac = hmac_md5(&response_key_nt, &lm_input);
        let mut lm_resp = Vec::with_capacity(24);
        lm_resp.extend_from_slice(&lm_mac);
        lm_resp.extend_from_slice(&client_challenge);
        (lm_resp, nt_resp)
    } else {
        let nt_hash = ntowfv1(&creds.password);
        if (challenge.flags & NTLMSSP_NEGOTIATE_EXTENDED_SESSIONSECURITY) != 0 {
            let mut lm_resp = vec![0u8; 24];
            lm_resp[0..8].copy_from_slice(&client_challenge);
            let mut concat = [0u8; 16];
            concat[0..8].copy_from_slice(&challenge.server_challenge);
            concat[8..16].copy_from_slice(&client_challenge);
            let session_hash = md5_bytes(&concat);
            let session_challenge: [u8; 8] = session_hash[0..8].try_into().unwrap();
            let nt_resp = desl_encrypt(&nt_hash, &session_challenge).to_vec();
            (lm_resp, nt_resp)
        } else {
            let nt_resp = desl_encrypt(&nt_hash, &challenge.server_challenge).to_vec();
            (nt_resp.clone(), nt_resp)
        }
    };
    let unicode = (challenge.flags & NTLMSSP_NEGOTIATE_UNICODE) != 0;
    let encode_str = |s: &str| -> Vec<u8> {
        if unicode {
            utf16le_bytes(s)
        } else {
            s.as_bytes().to_vec()
        }
    };
    let dom_bytes = encode_str(&creds.domain);
    let user_bytes = encode_str(&creds.username);
    let ws_bytes = encode_str(&creds.workstation);
    let header_len = 64usize;
    let dom_off = header_len;
    let user_off = dom_off + dom_bytes.len();
    let ws_off = user_off + user_bytes.len();
    let lm_off = ws_off + ws_bytes.len();
    let nt_off = lm_off + lm_response.len();
    let total_len = nt_off + nt_response.len();
    let out_flags = challenge.flags
        & (NTLMSSP_NEGOTIATE_UNICODE
            | NTLMSSP_NEGOTIATE_OEM
            | NTLMSSP_REQUEST_TARGET
            | NTLMSSP_NEGOTIATE_NTLM
            | NTLMSSP_NEGOTIATE_ALWAYS_SIGN
            | NTLMSSP_NEGOTIATE_EXTENDED_SESSIONSECURITY
            | NTLMSSP_NEGOTIATE_TARGET_INFO
            | NTLMSSP_NEGOTIATE_128
            | NTLMSSP_NEGOTIATE_56);
    let mut msg = Vec::with_capacity(total_len);
    msg.extend_from_slice(b"NTLMSSP\0");
    msg.extend_from_slice(&3u32.to_le_bytes());
    let push_sec_buf = |buf: &mut Vec<u8>, len: usize, off: usize| {
        let len16 = (len.min(u16::MAX as usize)) as u16;
        buf.extend_from_slice(&len16.to_le_bytes());
        buf.extend_from_slice(&len16.to_le_bytes());
        buf.extend_from_slice(&(off as u32).to_le_bytes());
    };
    push_sec_buf(&mut msg, lm_response.len(), lm_off);
    push_sec_buf(&mut msg, nt_response.len(), nt_off);
    push_sec_buf(&mut msg, dom_bytes.len(), dom_off);
    push_sec_buf(&mut msg, user_bytes.len(), user_off);
    push_sec_buf(&mut msg, ws_bytes.len(), ws_off);
    push_sec_buf(&mut msg, 0, total_len);
    msg.extend_from_slice(&out_flags.to_le_bytes());
    msg.extend_from_slice(&dom_bytes);
    msg.extend_from_slice(&user_bytes);
    msg.extend_from_slice(&ws_bytes);
    msg.extend_from_slice(&lm_response);
    msg.extend_from_slice(&nt_response);
    format!("NTLM {}", BASE64.encode(&msg))
}

static AUTH_NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_cnonce_and_challenge() -> (String, [u8; 8], u64) {
    let seq = AUTH_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let windows_ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_add(11_644_473_600)
        .saturating_mul(10_000_000);
    let seed = md5_bytes(&format!("{seq}:{now_ns}").as_bytes());
    let client_challenge: [u8; 8] = seed[0..8].try_into().unwrap();
    let cnonce = to_hex_lower(&client_challenge);
    (cnonce, client_challenge, windows_ticks)
}

pub fn request_uri_path_and_query(url: &str) -> String {
    if let Ok(parsed) = rama::net::uri::Uri::try_from(url) {
        let mut buf = rama::bytes::BytesMut::new();
        if parsed.write_http_origin_form(&mut buf).is_ok()
            && let Ok(text) = std::str::from_utf8(&buf)
            && !text.is_empty()
        {
            return text.to_owned();
        }
    }
    "/".into()
}

pub fn set_or_replace_header(headers: &mut Vec<Header>, name: &str, value: String) {
    headers.retain(|h| !h.name.eq_ignore_ascii_case(name));
    headers.push(Header {
        name: name.to_owned(),
        value: value.into_bytes(),
    });
}

pub fn find_authenticate_headers(
    response: &HttpResponse,
    header_name: &str,
) -> Vec<String> {
    response
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(header_name))
        .filter_map(|h| std::str::from_utf8(&h.value).ok())
        .map(|s| s.trim().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc1321_md5_and_rfc1320_md4_and_des_vectors_match() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            to_hex_lower(&ntowfv1("Password")),
            "a4f49c406510bdcab6824ee7c30fd852"
        );
        let challenge = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let nt_hash = ntowfv1("Password");
        let resp_v1 = desl_encrypt(&nt_hash, &challenge);
        assert_eq!(
            to_hex_lower(&resp_v1),
            "67c43011f30298a2ad35ece64f16331c44bdbed927841f94"
        );
    }

    #[test]
    fn digest_md5_and_sha256_challenge_response_computation() {
        let challenge = DigestChallenge::parse_from_header(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        assert_eq!(challenge.realm, "testrealm@host.com");
        assert_eq!(challenge.qop.as_deref(), Some("auth"));
        let header = challenge.build_authorization_value(
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            b"",
            1,
            "0a4f113b",
        );
        assert!(header.contains(r#"response="6629fae49393a05397450978507c4ef1""#));

        let sha256_chal = DigestChallenge::parse_from_header(
            r#"Digest realm="api", qop="auth", algorithm=SHA-256, nonce="abc123""#,
        )
        .unwrap();
        let sha_hdr = sha256_chal.build_authorization_value(
            "alice",
            "secret",
            "POST",
            "/v1/login",
            b"{}",
            1,
            "cn1",
        );
        assert!(sha_hdr.contains("algorithm=SHA-256"));
        assert!(sha_hdr.contains("response=\""));
    }

    #[test]
    fn ntlm_v1_and_v2_handshake_roundtrip() {
        let type1 = build_ntlm_type1_header_value("CORP", "WORKSTATION1");
        assert!(type1.starts_with("NTLM "));
        let type1_raw = BASE64.decode(type1[5..].as_bytes()).unwrap();
        assert_eq!(&type1_raw[0..8], b"NTLMSSP\0");
        assert_eq!(u32::from_le_bytes(type1_raw[8..12].try_into().unwrap()), 1);

        let mut type2_raw = Vec::new();
        type2_raw.extend_from_slice(b"NTLMSSP\0");
        type2_raw.extend_from_slice(&2u32.to_le_bytes());
        type2_raw.extend_from_slice(&0u16.to_le_bytes());
        type2_raw.extend_from_slice(&0u16.to_le_bytes());
        type2_raw.extend_from_slice(&48u32.to_le_bytes());
        let flags = NTLMSSP_NEGOTIATE_UNICODE
            | NTLMSSP_NEGOTIATE_NTLM
            | NTLMSSP_NEGOTIATE_EXTENDED_SESSIONSECURITY;
        type2_raw.extend_from_slice(&flags.to_le_bytes());
        type2_raw.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        type2_raw.extend_from_slice(&[0u8; 16]);
        let type2_hdr = format!("NTLM {}", BASE64.encode(&type2_raw));
        let parsed = NtlmType2Challenge::parse_from_header(&type2_hdr).unwrap();
        assert_eq!(parsed.server_challenge, [1, 2, 3, 4, 5, 6, 7, 8]);

        let creds = AuthCredentials {
            scheme: HttpAuthScheme::NtlmV2,
            username: "User".into(),
            password: "Password".into(),
            domain: "Domain".into(),
            workstation: "COMPUTER".into(),
        };
        let type3_v1 =
            build_ntlm_type3_header_value(&parsed, &creds, false, [0xaa; 8], 130_000_000_000);
        let type3_v2 =
            build_ntlm_type3_header_value(&parsed, &creds, true, [0xaa; 8], 130_000_000_000);
        assert!(type3_v1.starts_with("NTLM ") && type3_v2.starts_with("NTLM "));
        assert_ne!(type3_v1, type3_v2);
    }
}
