use std::io::{self, BufRead, Read};

use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};
use flate2::{Decompress, FlushDecompress, Status, bufread::MultiGzDecoder};

use super::{CHUNK, Coding, UnpackError};

pub(super) fn decode_layer(
    coding: Coding,
    input: &[u8],
    max: usize,
    check: &impl Fn() -> Result<(), UnpackError>,
) -> Result<Vec<u8>, UnpackError> {
    check()?;
    if input.len() > max {
        return Err(UnpackError::Failed(format!("编码正文超过 {max} 字节限额")));
    }
    match coding {
        Coding::Identity => Ok(input.to_vec()),
        Coding::Gzip => decode_gzip(input, max, check),
        Coding::Deflate => decode_deflate(input, max, check),
        Coding::Brotli => decode_brotli(input, max, check),
    }
}

fn output_space(output: &[u8], max: usize) -> usize {
    // A full output still needs an overflow probe and checksum/end-marker validation.
    CHUNK.min(max.saturating_sub(output.len()).saturating_add(1))
}

fn append_output(output: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<(), UnpackError> {
    if bytes.len() > max.saturating_sub(output.len()) {
        return Err(UnpackError::Failed(format!("解包正文超过 {max} 字节限额")));
    }
    let needed = output.len() + bytes.len();
    if needed > output.capacity() {
        let capacity = output.capacity().saturating_mul(2).max(needed).min(max);
        output
            .try_reserve_exact(capacity - output.len())
            .map_err(|_| UnpackError::Failed("解包正文内存分配失败".into()))?;
    }
    output.extend_from_slice(bytes);
    Ok(())
}

struct CheckedInput<'a, F> {
    remaining: &'a [u8],
    check: &'a F,
}

impl<F: Fn() -> Result<(), UnpackError>> Read for CheckedInput<'_, F> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let input = self.fill_buf()?;
        let count = input.len().min(output.len());
        output[..count].copy_from_slice(&input[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl<F: Fn() -> Result<(), UnpackError>> BufRead for CheckedInput<'_, F> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        (self.check)().map_err(io::Error::other)?;
        Ok(&self.remaining[..self.remaining.len().min(CHUNK)])
    }

    fn consume(&mut self, count: usize) {
        self.remaining = &self.remaining[count..];
    }
}

fn decode_gzip(
    input: &[u8],
    max: usize,
    check: &impl Fn() -> Result<(), UnpackError>,
) -> Result<Vec<u8>, UnpackError> {
    let mut decoder = MultiGzDecoder::new(CheckedInput {
        remaining: input,
        check,
    });
    let mut output = Vec::new();
    let mut buffer = [0u8; CHUNK];
    loop {
        check()?;
        let space = output_space(&output, max);
        let count = decoder.read(&mut buffer[..space]).map_err(|error| {
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<UnpackError>())
                .cloned()
                .unwrap_or_else(|| UnpackError::Failed("gzip 数据无效、不完整或含尾随数据".into()))
        })?;
        if count == 0 {
            return if decoder.get_ref().remaining.is_empty() {
                Ok(output)
            } else {
                Err(UnpackError::Failed("gzip 含尾随数据".into()))
            };
        }
        append_output(&mut output, &buffer[..count], max)?;
    }
}

fn decode_deflate(
    input: &[u8],
    max: usize,
    check: &impl Fn() -> Result<(), UnpackError>,
) -> Result<Vec<u8>, UnpackError> {
    let mut decoder = Decompress::new(true);
    let mut offset = 0usize;
    let mut output = Vec::new();
    let mut buffer = [0u8; CHUNK];
    loop {
        check()?;
        let end = offset + (input.len() - offset).min(CHUNK);
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let space = output_space(&output, max);
        let status = decoder
            .decompress(
                &input[offset..end],
                &mut buffer[..space],
                FlushDecompress::None,
            )
            .map_err(|_| UnpackError::Failed("zlib 数据或校验和无效".into()))?;
        let consumed = (decoder.total_in() - before_in) as usize;
        let produced = (decoder.total_out() - before_out) as usize;
        offset += consumed;
        append_output(&mut output, &buffer[..produced], max)?;
        if status == Status::StreamEnd {
            return if offset == input.len() {
                Ok(output)
            } else {
                Err(UnpackError::Failed("zlib 含尾随数据".into()))
            };
        }
        if consumed == 0 && produced == 0 {
            return Err(UnpackError::Failed("zlib 数据不完整或解码无进展".into()));
        }
    }
}

fn decode_brotli(
    input: &[u8],
    max: usize,
    check: &impl Fn() -> Result<(), UnpackError>,
) -> Result<Vec<u8>, UnpackError> {
    let mut state = BrotliState::new_strict(
        StandardAlloc::default(),
        StandardAlloc::default(),
        StandardAlloc::default(),
    );
    let mut offset = 0usize;
    let mut total_out = 0usize;
    let mut output = Vec::new();
    let mut buffer = [0u8; CHUNK];
    loop {
        check()?;
        let end = offset + (input.len() - offset).min(CHUNK);
        let mut available_in = end - offset;
        let mut consumed = 0usize;
        let space = output_space(&output, max);
        let mut available_out = space;
        let mut produced = 0usize;
        let status = BrotliDecompressStream(
            &mut available_in,
            &mut consumed,
            &input[offset..end],
            &mut available_out,
            &mut produced,
            &mut buffer[..space],
            &mut total_out,
            &mut state,
        );
        offset += consumed;
        append_output(&mut output, &buffer[..produced], max)?;
        match status {
            BrotliResult::ResultSuccess => {
                return if offset == input.len() {
                    Ok(output)
                } else {
                    Err(UnpackError::Failed("Brotli 含尾随数据".into()))
                };
            }
            BrotliResult::ResultFailure => {
                return Err(UnpackError::Failed("Brotli 数据无效".into()));
            }
            BrotliResult::NeedsMoreInput | BrotliResult::NeedsMoreOutput => {
                if consumed == 0 && produced == 0 {
                    return Err(UnpackError::Failed("Brotli 数据不完整或解码无进展".into()));
                }
            }
        }
    }
}
