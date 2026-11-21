use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use chrono::{DateTime, Datelike as _, Local, NaiveDate, TimeDelta, Timelike as _};

const MAX_HTML_ENTITY_BYTES: usize = 32;

/// Computes the SHA-256 digest of `input`.
pub fn sha256_digest(input: &[u8]) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(input).into()
}

/// Computes the SHA-384 digest of `input`.
pub fn sha384_digest(input: &[u8]) -> [u8; 48] {
    use sha2::{Digest as _, Sha384};
    Sha384::digest(input).into()
}

/// Computes the SHA-512 digest of `input`.
pub fn sha512_digest(input: &[u8]) -> [u8; 64] {
    use sha2::{Digest as _, Sha512};
    Sha512::digest(input).into()
}

/// Computes the SHA-1 digest of `input`.
pub fn sha1_digest(input: &[u8]) -> [u8; 20] {
    use sha1::{Digest as _, Sha1};
    Sha1::digest(input).into()
}

/// Computes the MD5 digest of `input`.
pub fn md5_digest(input: &[u8]) -> [u8; 16] {
    // Isolate `md5::Digest` (digest 0.10) from `sha1`/`sha2::Digest` (digest 0.11).
    use md5::{Digest as _, Md5};
    Md5::digest(input).into()
}

/// Computes RFC 2104 HMAC-SHA256 over `msg` using `key`.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    let mut block_key = [0u8; 64];
    if key.len() > 64 {
        block_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let ipad = block_key.map(|b| b ^ 0x36);
    let opad = block_key.map(|b| b ^ 0x5c);
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

/// Computes the IEEE CRC-32 checksum of `input`.
pub fn crc32_checksum(input: &[u8]) -> u32 {
    crc32fast::hash(input)
}

/// Encodes `input` as lowercase hexadecimal.
pub fn hex_encode(input: &[u8]) -> String {
    hex::encode(input)
}

/// Decodes ASCII hexadecimal bytes, ignoring ASCII whitespace.
pub fn hex_decode_bytes(input: &[u8]) -> Result<Vec<u8>> {
    let clean: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    hex::decode(&clean).map_err(|err| match err {
        hex::FromHexError::OddLength => anyhow::anyhow!("十六进制输入长度必须为偶数"),
        _ => anyhow::anyhow!("十六进制输入包含非法字符"),
    })
}

/// Decodes a hexadecimal string, ignoring ASCII whitespace.
pub fn hex_decode(input: &str) -> Result<Vec<u8>> {
    hex_decode_bytes(input.as_bytes())
}

/// Converts an ASCII hex digit byte into its numeric nibble `0..16`.
pub fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Encodes `input` using standard padded Base64.
pub fn base64_encode(input: &[u8]) -> String {
    STANDARD.encode(input)
}

/// Decodes canonical standard padded Base64 bytes.
pub fn base64_decode(input: &[u8]) -> Result<Vec<u8>> {
    STANDARD.decode(input).context("Base64 解码失败")
}

/// Encodes `input` using unpadded URL-safe Base64.
pub fn base64url_encode(input: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(input)
}

/// Decodes URL-safe Base64 bytes (padded or unpadded), ignoring ASCII whitespace.
pub fn base64url_decode(input: &[u8]) -> Result<Vec<u8>> {
    let trimmed: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    URL_SAFE_NO_PAD
        .decode(&trimmed)
        .or_else(|_| URL_SAFE.decode(&trimmed))
        .context("Base64URL 解码包含无效字符或填充")
}

/// Encodes `input` as standard or URL-safe Base64, with or without `=` padding.
pub fn base64_encode_variant(input: &[u8], url_safe: bool, padded: bool) -> String {
    match (url_safe, padded) {
        (false, true) => STANDARD.encode(input),
        (false, false) => STANDARD_NO_PAD.encode(input),
        (true, true) => URL_SAFE.encode(input),
        (true, false) => URL_SAFE_NO_PAD.encode(input),
    }
}

/// Decodes canonical Base64 bytes (`len >= 4`, canonical padding/trailing bits) and reports whether input was padded.
pub fn base64_decode_canonical(input: &[u8], url_safe: bool) -> Option<(Vec<u8>, bool)> {
    if input.len() < 4 {
        return None;
    }
    let padded = input.ends_with(b"=");
    let decoded = match (url_safe, padded) {
        (false, true) => STANDARD.decode(input).ok()?,
        (false, false) => STANDARD_NO_PAD.decode(input).ok()?,
        (true, true) => URL_SAFE.decode(input).ok()?,
        (true, false) => URL_SAFE_NO_PAD.decode(input).ok()?,
    };
    Some((decoded, padded))
}

/// HTML-encodes `&`, `<`, `>`, `"`, and `'` (`&#x27;`) in arbitrary byte slices.
pub fn html_encode_bytes(input: &[u8]) -> Vec<u8> {
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

/// HTML-encodes `&`, `<`, `>`, `"`, and `'` (`&#x27;`) in a UTF-8 string.
pub fn html_encode(input: &str) -> String {
    String::from_utf8(html_encode_bytes(input.as_bytes()))
        .expect("UTF-8 input with ASCII entity replacements remains valid UTF-8")
}

/// Decodes a single HTML entity body (between `&` and `;`).
pub fn decode_html_entity(entity: &str) -> Option<char> {
    if entity.eq_ignore_ascii_case("amp") {
        return Some('&');
    }
    if entity.eq_ignore_ascii_case("lt") {
        return Some('<');
    }
    if entity.eq_ignore_ascii_case("gt") {
        return Some('>');
    }
    if entity.eq_ignore_ascii_case("quot") {
        return Some('"');
    }
    if entity == "apos" {
        return Some('\'');
    }
    if entity.eq_ignore_ascii_case("nbsp") {
        return Some('\u{00a0}');
    }
    let numeric = entity.strip_prefix('#')?;
    let codepoint = if let Some(hex) = numeric
        .strip_prefix('x')
        .or_else(|| numeric.strip_prefix('X'))
    {
        if hex.is_empty() || hex.len() > 8 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()?
    } else {
        if numeric.is_empty() || numeric.len() > 8 || !numeric.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        numeric.parse::<u32>().ok()?
    };
    if codepoint == 0 {
        return None;
    }
    char::from_u32(codepoint)
}

/// Decodes common named and numeric HTML entities in arbitrary byte slices, preserving non-entity bytes verbatim.
pub fn html_decode_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        if input[idx] == b'&' {
            let after_amp = &input[idx + 1..];
            let window = &after_amp[..after_amp.len().min(MAX_HTML_ENTITY_BYTES)];
            if let Some(semi) = window.iter().position(|&b| b == b';') {
                let candidate = &window[..semi];
                if !candidate.contains(&b'&')
                    && let Some(ch) = std::str::from_utf8(candidate)
                        .ok()
                        .and_then(decode_html_entity)
                {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    idx += 1 + semi + 1;
                    continue;
                }
            }
        }
        out.push(input[idx]);
        idx += 1;
    }
    out
}

/// Decodes HTML entities in `input`, invoking cooperative `check` periodically.
pub fn html_decode_checked(input: &str, check: &impl Fn() -> Result<()>) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    let mut steps = 0_usize;
    while let Some(amp) = rest.find('&') {
        steps = steps.wrapping_add(1);
        if steps & 0x3ff == 0 {
            check()?;
        }
        out.push_str(&rest[..amp]);
        let after_amp = &rest[amp + 1..];
        let window = &after_amp[..after_amp.floor_char_boundary(MAX_HTML_ENTITY_BYTES)];
        if let Some(semi) = window.find(';') {
            let candidate = &window[..semi];
            if !candidate.contains('&')
                && let Some(decoded) = decode_html_entity(candidate)
            {
                out.push(decoded);
                rest = &after_amp[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after_amp;
    }
    out.push_str(rest);
    check()?;
    Ok(out)
}

/// Decodes common named and numeric HTML entities in `input`.
pub fn html_decode(input: &str) -> String {
    html_decode_checked(input, &|| Ok(())).expect("infallible check cannot fail")
}

/// Returns current wall-clock time as Unix epoch milliseconds clamped to `i64::MAX`.
pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

/// Formats `unix_ms` as a UTC ISO-8601 timestamp (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
pub fn format_utc_iso8601(unix_ms: i64) -> String {
    let (year, month, day, hour, minute, second, millis): (i64, u32, u32, u32, u32, u32, u32) =
        if let Some(dt) = DateTime::from_timestamp_millis(unix_ms) {
            (
                i64::from(dt.year()),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second(),
                dt.timestamp_subsec_millis(),
            )
        } else {
            // Fold astronomical timestamps into the exact 400-year Gregorian cycle (146,097 days).
            let seconds = unix_ms.div_euclid(1000);
            let millis = unix_ms.rem_euclid(1000) as u32;
            let days = seconds.div_euclid(86_400);
            let day_secs = seconds.rem_euclid(86_400) as u32;
            let cycles = days.div_euclid(146_097);
            let rem_days = days.rem_euclid(146_097);
            let base =
                NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is valid")
                    + TimeDelta::days(rem_days);
            (
                i64::from(base.year()) + cycles * 400,
                base.month(),
                base.day(),
                day_secs / 3600,
                (day_secs % 3600) / 60,
                day_secs % 60,
                millis,
            )
        };
    let year_text = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:04}", year.unsigned_abs())
    } else {
        format!("+{year}")
    };
    format!("{year_text}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Converts `unix_secs` to local calendar `(year, month, day, hour, minute, second)`.
pub fn local_datetime_parts(unix_secs: i64) -> Option<(i32, u32, u32, u32, u32, u32)> {
    let utc = DateTime::from_timestamp(unix_secs, 0)?;
    let local = utc.with_timezone(&Local);
    Some((
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
    ))
}

/// Returns local `(hour, minute)` for `unix_secs`, falling back to UTC time-of-day if out of range.
pub fn local_hh_mm(unix_secs: i64) -> (u32, u32) {
    if let Some((_, _, _, hour, minute, _)) = local_datetime_parts(unix_secs) {
        return (hour, minute);
    }
    let day_secs = unix_secs.rem_euclid(86_400) as u32;
    (day_secs / 3600, (day_secs % 3600) / 60)
}

/// Formats the current local time as `HH:MM`.
pub fn format_local_hh_mm() -> String {
    let now = Local::now();
    format!("{:02}:{:02}", now.hour(), now.minute())
}

/// Converts `unix_secs` to local calendar `(year, month, day)`.
pub fn local_date_parts(unix_secs: i64) -> Option<(i32, u32, u32)> {
    local_datetime_parts(unix_secs).map(|(year, month, day, _, _, _)| (year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_and_hmac_match_standard_vectors() {
        assert_eq!(
            hex_encode(&sha256_digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_encode(&sha384_digest(b"abc")),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            hex_encode(&sha512_digest(b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hex_encode(&sha1_digest(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex_encode(&md5_digest(b"abc")),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(crc32_checksum(b"123456789"), 0xcbf4_3926);

        assert_eq!(
            hex_encode(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let long_key = vec![0xaa; 131];
        assert_eq!(
            hex_encode(&hmac_sha256(&long_key, b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn hex_and_base64_variants_roundtrip_and_validate() {
        let raw = b"\x00hello\xffworld";
        let hx = hex_encode(raw);
        assert_eq!(hex_decode(&format!("  {} \n", hx.to_uppercase())).unwrap(), raw);
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());

        let b64 = base64_encode(raw);
        assert_eq!(base64_decode(b64.as_bytes()).unwrap(), raw);
        assert!(base64_decode(b"Zh==").is_err());
        assert!(base64_decode(b"Zg").is_err());

        let b64u = base64url_encode(b"\xfb\xff\xfe");
        assert_eq!(b64u, "-__-");
        assert_eq!(base64url_decode(b"-__-\n").unwrap(), b"\xfb\xff\xfe");
        assert_eq!(base64url_decode(b"-__-==").unwrap_or_default(), Vec::<u8>::new());
        assert_eq!(base64url_decode(b"Zm8=").unwrap(), b"fo");

        for url_safe in [false, true] {
            for padded in [false, true] {
                let encoded = base64_encode_variant(b"{\"user\":\"alice\"}", url_safe, padded);
                let (decoded, seen_padded) = base64_decode_canonical(encoded.as_bytes(), url_safe).unwrap();
                assert_eq!(decoded, b"{\"user\":\"alice\"}");
                assert_eq!(seen_padded, padded);
            }
        }
        assert!(base64_decode_canonical(b"abc", false).is_none());
        assert!(base64_decode_canonical(b"Zh==", false).is_none());
    }

    #[test]
    fn html_codec_preserves_non_utf8_and_decodes_entities() {
        let encoded = html_encode("<a href=\"x\">'&'</a>");
        assert_eq!(encoded, "&lt;a href=&quot;x&quot;&gt;&#x27;&amp;&#x27;&lt;/a&gt;");
        assert_eq!(
            html_decode(&format!("{encoded} &apos; &#65; &#x42; &nbsp; &#0; &unknown;")),
            "<a href=\"x\">'&'</a> ' A B \u{00a0} &#0; &unknown;"
        );
        assert_eq!(html_encode_bytes(b"\xff<&'\xfe"), b"\xff&lt;&amp;&#x27;\xfe");
        assert_eq!(html_decode_bytes(b"\xff&lt;&#x27;\xfe"), b"\xff<'\xfe");
    }

    #[test]
    fn time_and_civil_date_helpers_handle_boundaries() {
        assert!(now_unix_ms() > 0);
        assert_eq!(format_utc_iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_utc_iso8601(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(format_utc_iso8601(-1_000), "1969-12-31T23:59:59.000Z");
        assert_eq!(format_utc_iso8601(-86_400_000), "1969-12-31T00:00:00.000Z");
        assert_eq!(format_utc_iso8601(951_782_400_005), "2000-02-29T00:00:00.005Z");
        assert_eq!(format_utc_iso8601(253_402_300_799_999), "9999-12-31T23:59:59.999Z");
        assert!(format_utc_iso8601(i64::MIN).ends_with('Z'));
        assert!(format_utc_iso8601(i64::MAX).ends_with('Z'));

        let hh_mm = format_local_hh_mm();
        assert_eq!(hh_mm.len(), 5);
        assert_eq!(&hh_mm[2..3], ":");
        assert!(local_date_parts(1_700_000_000).is_some());
        assert!(local_datetime_parts(i64::MIN / 1000).is_none());
        assert!(local_datetime_parts(i64::MAX / 1000).is_none());
    }
}
