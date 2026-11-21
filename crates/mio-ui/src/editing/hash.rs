pub(crate) fn sha256_digest(input: &[u8]) -> [u8; 32] {
    mio_core::codec_util::sha256_digest(input)
}

#[cfg(test)]
pub(crate) fn sha256_hex(input: &[u8]) -> String {
    mio_core::codec_util::hex_encode(&sha256_digest(input))
}

pub(crate) fn md5_digest(input: &[u8]) -> [u8; 16] {
    mio_core::codec_util::md5_digest(input)
}

#[cfg(test)]
pub(crate) fn md5_hex(input: &[u8]) -> String {
    mio_core::codec_util::hex_encode(&md5_digest(input))
}
