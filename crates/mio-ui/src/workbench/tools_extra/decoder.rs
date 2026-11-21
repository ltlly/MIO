pub(super) const MAX_DECODER_BYTES: usize = 512 * 1024;
pub(super) const MAX_DECODER_STEPS: usize = 16;
const STEP_PREVIEW_CHARS: usize = 36;

use super::*;
mod codecs;
mod render;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum DecoderOp {
    Convert(editing::ConvertKind),
    Smart,
    GzipCompress,
    GzipDecompress,
    DeflateCompress,
    DeflateDecompress,
    BrotliDecompress,
    Octal(bool),
    Binary(bool),
    Hash(&'static str),
}

fn decode_compressed_candidate(bytes: &[u8], text: &str, decode: impl Fn(&[u8]) -> Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
    decode(bytes)
        .or_else(|first| editing::hex_decode(text).and_then(|raw| decode(&raw)).map_err(|_| first))
        .or_else(|first| editing::base64_decode_bytes(text).and_then(|raw| decode(&raw)).map_err(|_| first))
}

impl DecoderOp {
    fn apply(self, bytes: &[u8], text: &str) -> Result<(Vec<u8>, Option<String>), String> {
        let output = match self {
            Self::Convert(kind) => editing::convert_bytes(kind, bytes, text)?,
            Self::Smart => editing::smart_decode_bytes(bytes, MAX_DECODER_BYTES)?.0,
            Self::GzipCompress => mio_core::compress_gzip_bytes(bytes, MAX_DECODER_BYTES)?,
            Self::GzipDecompress => decode_compressed_candidate(bytes, text, |raw| mio_core::decompress_gzip_bytes(raw, MAX_DECODER_BYTES))?,
            Self::DeflateCompress => mio_core::compress_deflate_bytes(bytes, MAX_DECODER_BYTES)?,
            Self::DeflateDecompress => decode_compressed_candidate(bytes, text, |raw| mio_core::decompress_deflate_bytes(raw, MAX_DECODER_BYTES))?,
            Self::BrotliDecompress => decode_compressed_candidate(bytes, text, |raw| mio_core::decompress_brotli_bytes(raw, MAX_DECODER_BYTES))?,
            Self::Octal(true) => codecs::octal_encode_bytes(bytes, MAX_DECODER_BYTES)?,
            Self::Octal(false) => codecs::octal_decode_bytes(text)?,
            Self::Binary(true) => codecs::binary_encode_bytes(bytes, MAX_DECODER_BYTES)?,
            Self::Binary(false) => codecs::binary_decode_bytes(text)?,
            Self::Hash(kind) => {
                let digest = match kind {
                    "sha256" => mio_core::codec_util::sha256_digest(bytes).to_vec(),
                    "sha384" => mio_core::codec_util::sha384_digest(bytes).to_vec(),
                    "sha512" => mio_core::codec_util::sha512_digest(bytes).to_vec(),
                    "sha1" => mio_core::codec_util::sha1_digest(bytes).to_vec(),
                    "crc32" => mio_core::codec_util::crc32_checksum(bytes)
                        .to_be_bytes()
                        .to_vec(),
                    "md5" => mio_core::codec_util::md5_digest(bytes).to_vec(),
                    _ => unreachable!("unsupported Decoder hash"),
                };
                let hex = mio_core::codec_util::hex_encode(&digest);
                return Ok((digest, Some(hex)));
            }
        };
        if output.len() > MAX_DECODER_BYTES {
            return Err("Decoder 输出超过 512 KiB 上限。".into());
        }
        Ok((output, None))
    }
}

impl Workbench {
    pub(in crate::workbench) fn on_decoder_input_edited(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let revision = self.extra_tools.decoder_input.read(cx).document_revision();
        if revision == self.extra_tools.decoder_input_revision {
            return;
        }
        self.extra_tools.decoder_input_revision = revision;
        let operations: Option<Vec<_>> = self.extra_tools.steps
            [self.extra_tools.decoder_chain_start.min(self.extra_tools.steps.len())..]
            .iter()
            .map(|step| step.op)
            .collect();
        let Some(operations) = operations.filter(|operations| !operations.is_empty()) else {
            return;
        };
        let replay = (|| {
            let (mut bytes, mut text) = self.decoder_current_input(window, cx)?;
            let mut updates = Vec::new();
            for operation in operations {
                let (output, display) = operation.apply(&bytes, &text)?;
                let display = display.unwrap_or_else(|| match std::str::from_utf8(&output) {
                    Ok(text) => text.to_owned(),
                    Err(_) => editing::hex_encode(&output),
                });
                updates.push((output.clone(), display.clone()));
                text = display;
                bytes = output;
            }
            Ok::<_, String>(updates)
        })();
        match replay {
            Ok(updates) => {
                for (step, (bytes, display)) in self.extra_tools.steps
                    [self.extra_tools.decoder_chain_start..].iter_mut().zip(updates)
                {
                    step.raw_bytes = bytes;
                    step.output_preview = display.chars().take(STEP_PREVIEW_CHARS).collect();
                    step.full_text = display;
                }
                let step = self.extra_tools.steps.last().unwrap().clone();
                self.extra_tools.decoder_output_bytes = Some(step.raw_bytes.clone());
                let display = if self.extra_tools.decoder_output_hex_view {
                    editing::hex_encode(&step.raw_bytes)
                } else {
                    step.full_text
                };
                self.extra_tools.decoder_output.update(cx, |state, cx| {
                    state.set_value(display, window, cx)
                });
                self.error = None;
            }
            Err(error) => {
                self.extra_tools.decoder_output_bytes = None;
                self.extra_tools.decoder_output.update(cx, |state, cx| {
                    state.set_value("", window, cx)
                });
                self.fail(error, cx);
            }
        }
        cx.notify();
    }

    fn decoder_operation(
        &mut self,
        operation: DecoderOp,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.decoder_current_input(window, cx)
            .and_then(|(bytes, text)| operation.apply(&bytes, &text));
        match result {
            Ok((bytes, display)) => {
                self.apply_decoder_output(bytes, display, label, window, cx);
                self.extra_tools.steps.last_mut().unwrap().op = Some(operation);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(super) fn set_decoder_hex_view(&mut self, output: bool, hex_view: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = if output { self.extra_tools.decoder_output_hex_view } else { self.extra_tools.decoder_input_hex_view };
        if current == hex_view {
            return;
        }
        let result = if output {
            self.extra_tools.decoder_output_bytes.clone().ok_or_else(|| "Decoder 输出为空。".into())
        } else {
            self.decoder_current_input(window, cx).map(|(bytes, _)| bytes)
        };
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => { self.fail(error, cx); return; }
        };
        let display = if hex_view {
            editing::hex_encode(&bytes)
        } else {
            match String::from_utf8(bytes.clone()) {
                Ok(text) => text,
                Err(error) => { self.fail(format!("无法显示 UTF-8 文本：{error}"), cx); return; }
            }
        };
        if output {
            self.extra_tools.decoder_output_hex_view = hex_view;
            self.extra_tools.decoder_output.update(cx, |state, cx| state.set_value(display, window, cx));
        } else {
            self.extra_tools.decoder_input_hex_view = hex_view;
            self.extra_tools.decoder_input.update(cx, |state, cx| state.set_value(display, window, cx));
            let revision = self.extra_tools.decoder_input.read(cx).document_revision();
            self.extra_tools.decoder_input_revision = revision;
            self.extra_tools.decoder_input_bytes = Some((revision, bytes));
        }
        cx.notify();
    }

    pub(super) fn continue_decoder_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = editing::snapshot(&self.extra_tools.decoder_input, window, cx) {
            self.fail(error, cx);
            return;
        }
        let output = self.extra_tools.decoder_output.read(cx).value();
        if output.is_empty() {
            self.fail("Decoder 输出为空，无法继续转换。", cx);
            return;
        }
        let out_bytes = self.extra_tools.decoder_output_bytes.clone();
        if out_bytes.as_ref().is_some_and(|bytes| bytes.len() > MAX_DECODER_BYTES)
            || (out_bytes.is_none() && output.len() > MAX_DECODER_BYTES)
        {
            self.fail("Decoder 输出超过字节上限，无法继续转换。", cx);
            return;
        }
        self.extra_tools.decoder_input_hex_view = out_bytes.as_ref().is_some_and(|bytes| std::str::from_utf8(bytes).is_err());
        self.extra_tools.decoder_input.update(cx, |state, cx| state.set_value(output, window, cx));
        let revision = self.extra_tools.decoder_input.read(cx).document_revision();
        self.extra_tools.decoder_input_revision = revision;
        self.extra_tools.decoder_input_bytes = out_bytes.map(|bytes| (revision, bytes));
        self.extra_tools.decoder_output.update(cx, |state, cx| state.set_value("", window, cx));
        self.extra_tools.decoder_output_bytes = None;
        self.extra_tools.decoder_output_hex_view = false;
        self.extra_tools.decoder_chain_continuing = !self.extra_tools.steps.is_empty();
        cx.notify();
    }
    pub(in crate::workbench) fn send_to_decoder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        if let Err(error) = editing::snapshot(&self.extra_tools.decoder_input, window, cx) {
            self.fail(error, cx);
            return;
        }
        let payload = self
            .focused_tool_selection(window, cx)
            .and_then(|selected| match selected {
                Some(text) => Ok((text.as_bytes().to_vec(), Some(text))),
                None => self.tool_message_bytes(false, window, cx).map(|bytes| {
                    let text = String::from_utf8(bytes.clone()).ok();
                    (bytes, text)
                }),
            });
        match payload {
            Ok((bytes, maybe_text)) => {
                if bytes.len() > MAX_DECODER_BYTES {
                    self.fail(
                        format!(
                            "Decoder 输入超过 {} KiB 上限，未覆盖原内容。",
                            MAX_DECODER_BYTES / 1024
                        ),
                        cx,
                    );
                    return;
                }
                let (display, raw_backup) = match maybe_text {
                    Some(text) => (text, None),
                    None => (editing::hex_encode(&bytes), Some(bytes)),
                };
                self.extra_tools
                    .decoder_input
                    .update(cx, |state, cx| state.set_value(display, window, cx));
                let rev = self.extra_tools.decoder_input.read(cx).document_revision();
                self.extra_tools.decoder_input_revision = rev;
                self.extra_tools.decoder_input_hex_view = raw_backup.is_some();
                self.extra_tools.decoder_input_bytes = raw_backup.map(|b| (rev, b));
                self.extra_tools
                    .decoder_output
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.extra_tools.decoder_output_bytes = None;
                self.extra_tools.decoder_mode = "".into();
                self.extra_tools.steps.clear();
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Decoder;
                self.focus.focus(window, cx);
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    fn decoder_current_input(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(Vec<u8>, String), String> {
        let text = editing::snapshot(&self.extra_tools.decoder_input, window, cx)
            .map_err(str::to_owned)?
            .to_string();
        let rev = self.extra_tools.decoder_input.read(cx).document_revision();
        if let Some((saved_rev, bytes)) = &self.extra_tools.decoder_input_bytes
            && *saved_rev == rev
        {
            return Ok((bytes.clone(), text));
        }
        if self.extra_tools.decoder_input_hex_view {
            return editing::hex_decode(&text).map(|bytes| (bytes, text));
        }
        if text.len() > MAX_DECODER_BYTES {
            return Err("Decoder 输入超过 512 KiB 上限。".into());
        }
        Ok((text.as_bytes().to_vec(), text))
    }

    fn apply_decoder_output(
        &mut self,
        bytes: Vec<u8>,
        display_override: Option<String>,
        mode: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if bytes.len() > MAX_DECODER_BYTES {
            self.fail(
                format!(
                    "Decoder 输出超过 {} KiB 上限，未执行转换。",
                    MAX_DECODER_BYTES / 1024
                ),
                cx,
            );
            return;
        }
        let mut mode_str = mode.into().to_string();
        let display = match display_override {
            Some(custom) => custom,
            None => match std::str::from_utf8(&bytes) {
                Ok(utf8) => {
                    if utf8.len() > MAX_DECODER_BYTES {
                        self.fail(
                            format!(
                                "Decoder 输出超过 {} KiB 上限，未执行转换。",
                                MAX_DECODER_BYTES / 1024
                            ),
                            cx,
                        );
                        return;
                    }
                    utf8.to_owned()
                }
                Err(_) => {
                    if !mode_str.contains("Hex") {
                        mode_str.push_str(" (Hex 视图，保留原始字节)");
                    }
                    editing::hex_encode(&bytes)
                }
            },
        };
        let preview: String = display
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .take(STEP_PREVIEW_CHARS)
            .collect();
        if self.extra_tools.steps.len() >= MAX_DECODER_STEPS {
            self.extra_tools.steps.remove(0);
            self.extra_tools.decoder_chain_start = self.extra_tools.decoder_chain_start.saturating_sub(1);
        }
        if !self.extra_tools.decoder_chain_continuing {
            self.extra_tools.decoder_chain_start = self.extra_tools.steps.len();
        }
        self.extra_tools.decoder_chain_continuing = false;
        self.extra_tools.decoder_input_revision = self.extra_tools.decoder_input.read(cx).document_revision();
        self.extra_tools.steps.push(DecoderStep {
            op: None,
            op_label: mode_str.clone(),
            output_preview: preview,
            full_text: display.clone(),
            raw_bytes: bytes.clone(),
        });
        self.extra_tools.decoder_mode = mode_str.into();
        self.extra_tools.decoder_output_hex_view = std::str::from_utf8(&bytes).is_err();
        self.extra_tools.decoder_output_bytes = Some(bytes);
        self.extra_tools
            .decoder_output
            .update(cx, |state, cx| state.set_value(display, window, cx));
        cx.notify();
    }

    fn restore_decoder_step(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = editing::snapshot(&self.extra_tools.decoder_input, window, cx) {
            self.fail(error, cx);
            return;
        }
        let Some(step) = self.extra_tools.steps.get(index).cloned() else {
            return;
        };
        self.extra_tools.steps.truncate(index + 1);
        self.extra_tools.decoder_input.update(cx, |state, cx| {
            state.set_value(step.full_text.clone(), window, cx)
        });
        let rev = self.extra_tools.decoder_input.read(cx).document_revision();
        self.extra_tools.decoder_input_revision = rev;
        self.extra_tools.decoder_chain_start = index + 1;
        self.extra_tools.decoder_chain_continuing = false;
        let hex_view = std::str::from_utf8(&step.raw_bytes).is_err();
        self.extra_tools.decoder_input_hex_view = hex_view;
        self.extra_tools.decoder_output_hex_view = hex_view;
        self.extra_tools.decoder_input_bytes = Some((rev, step.raw_bytes.clone()));
        self.extra_tools
            .decoder_output
            .update(cx, |state, cx| state.set_value(step.full_text, window, cx));
        self.extra_tools.decoder_output_bytes = Some(step.raw_bytes);
        self.extra_tools.decoder_mode = step.op_label.into();
        cx.notify();
    }

    fn clear_decoder_steps(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.extra_tools.steps.clear();
        self.extra_tools.decoder_chain_start = 0;
        self.extra_tools.decoder_chain_continuing = false;
        self.extra_tools.decoder_output_bytes = None;
        self.extra_tools.decoder_output_hex_view = false;
        self.extra_tools.decoder_mode = "".into();
        self.extra_tools
            .decoder_output
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn decoder_smart_decode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self
            .decoder_current_input(window, cx)
            .and_then(|(bytes, _)| editing::smart_decode_bytes(&bytes, MAX_DECODER_BYTES));
        match result {
            Ok((out_bytes, steps)) => {
                let mode = if steps.is_empty() {
                    "智能解码 (未检测到可解码层)".to_owned()
                } else {
                    format!("智能解码 ({})", steps.join(" → "))
                };
                self.apply_decoder_output(out_bytes, None, mode, window, cx);
                self.extra_tools.steps.last_mut().unwrap().op = Some(DecoderOp::Smart);
            }
            Err(error) => self.fail(error, cx),
        }
    }

}
