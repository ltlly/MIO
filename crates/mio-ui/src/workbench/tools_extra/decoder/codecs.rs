use std::fmt::Write;

pub(super) fn octal_encode_bytes(bytes: &[u8], max_bytes: usize) -> Result<Vec<u8>, String> {
    if bytes.len().saturating_mul(4) > max_bytes {
        return Err(format!(
            "Decoder 输出超过 {max_bytes} 字节上限，未执行转换。"
        ));
    }
    let mut out = String::with_capacity(bytes.len().saturating_mul(4));
    for (idx, &byte) in bytes.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{byte:03o}");
    }
    Ok(out.into_bytes())
}

pub(super) fn octal_decode_bytes(text: &str) -> Result<Vec<u8>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let cleaned = trimmed.replace("\\0", " ").replace('\\', " ");
    let has_sep = cleaned
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b == b',' || b == b';');
    if has_sep {
        let mut out = Vec::new();
        for token in cleaned.split(|c: char| c.is_ascii_whitespace() || c == ',' || c == ';') {
            if token.is_empty() {
                continue;
            }
            let token = token
                .strip_prefix("0o")
                .or_else(|| token.strip_prefix("0O"))
                .unwrap_or(token);
            if token.len() > 3 || !token.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
                return Err(format!("无效的八进制字节：{token}"));
            }
            let val =
                u16::from_str_radix(token, 8).map_err(|_| format!("无效的八进制字节：{token}"))?;
            let byte = u8::try_from(val)
                .map_err(|_| format!("八进制值超出单字节范围 (000–377)：{token}"))?;
            out.push(byte);
        }
        Ok(out)
    } else {
        let digits: Vec<u8> = trimmed.bytes().collect();
        if !digits.len().is_multiple_of(3) || !digits.iter().all(|b| (b'0'..=b'7').contains(b)) {
            return Err("连续八进制文本必须由每组 3 位八进制数字 (0–7) 组成。".into());
        }
        let mut out = Vec::with_capacity(digits.len() / 3);
        for chunk in digits.as_chunks::<3>().0 {
            let s = std::str::from_utf8(chunk).expect("ascii octal digits");
            let val = u16::from_str_radix(s, 8).expect("valid octal");
            let byte =
                u8::try_from(val).map_err(|_| format!("八进制值超出单字节范围 (000–377)：{s}"))?;
            out.push(byte);
        }
        Ok(out)
    }
}

pub(super) fn binary_encode_bytes(bytes: &[u8], max_bytes: usize) -> Result<Vec<u8>, String> {
    if bytes.len().saturating_mul(9) > max_bytes {
        return Err(format!(
            "Decoder 输出超过 {max_bytes} 字节上限，未执行转换。"
        ));
    }
    let mut out = String::with_capacity(bytes.len().saturating_mul(9));
    for (idx, &byte) in bytes.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{byte:08b}");
    }
    Ok(out.into_bytes())
}

pub(super) fn binary_decode_bytes(text: &str) -> Result<Vec<u8>, String> {
    let bits: Vec<u8> = text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'_')
        .collect();
    if bits.is_empty() {
        return Ok(Vec::new());
    }
    if bits.iter().any(|&b| b != b'0' && b != b'1') {
        return Err("Binary 解码仅允许包含 0、1 及空白分隔符。".into());
    }
    if !bits.len().is_multiple_of(8) {
        return Err("Binary 位串长度必须为 8 的整数倍。".into());
    }
    let mut out = Vec::with_capacity(bits.len() / 8);
    for chunk in bits.as_chunks::<8>().0 {
        let mut byte = 0u8;
        for &bit in chunk {
            byte = (byte << 1) | (bit - b'0');
        }
        out.push(byte);
    }
    Ok(out)
}
