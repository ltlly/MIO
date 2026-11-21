use mio_core::intruder::{
    BitFlipperConfig, BitFlipperFormat, BruteForcerConfig, CaseModificationConfig,
    CaseModificationMode, CaseVariantsConfig, CharacterBlocksConfig, CharacterFrobberConfig,
    CharacterSubstitutionConfig, CustomIteratorConfig, CustomIteratorSlot, DateOutputFormat,
    DateSequenceConfig, DateStepUnit, EcbBlockShufflerConfig, IllegalUnicodeConfig,
    IntruderPayloadType, NumbersGeneratorConfig, PayloadProcessingRule, RecursiveGrepConfig,
    RuntimeFileConfig, UsernameGeneratorConfig, generate_bit_flipper_payloads,
    generate_brute_forcer_payloads, generate_case_modification_payloads,
    generate_case_variant_payloads, generate_character_block_payloads,
    generate_character_frobber_payloads, generate_character_substitution_payloads,
    generate_custom_iterator_payloads, generate_date_sequence_payloads,
    generate_dates_with_unit_payloads, generate_ecb_block_shuffler_payloads,
    generate_illegal_unicode_payloads, generate_numbers_config_payloads,
    generate_recursive_grep_payloads, generate_runtime_file_payloads, generate_username_payloads,
    generators::{EmptyRepeatConfig, generate_empty_repeats},
};

use super::*;

pub(super) fn parse_empty_repeat_quick_config(raw: &str) -> Result<EmptyRepeatConfig, String> {
    let trimmed = raw.trim();
    let count = if trimmed.is_empty() {
        10
    } else {
        trimmed
            .parse::<usize>()
            .map_err(|_| "重复基准次数须为 1–100000 之间的正整数。".to_owned())?
    };
    Ok(EmptyRepeatConfig { count })
}

pub(super) fn parse_date_quick_config(
    raw: &str,
    format: DateOutputFormat,
) -> Result<DateSequenceConfig, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(DateSequenceConfig {
            from_date: "2026-01-01".into(),
            to_date: "2026-01-07".into(),
            step_days: 1,
            format,
        });
    }
    let (range_part, step_days) = match trimmed.rsplit_once(':') {
        Some((head, step_raw)) => {
            let step = step_raw
                .trim()
                .parse::<u16>()
                .map_err(|_| "日期步长须为 1–366 之间的正整数。".to_owned())?;
            (head.trim(), step)
        }
        None => (trimmed, 1),
    };
    let Some((from, to)) = range_part
        .split_once("..")
        .or_else(|| range_part.split_once('~'))
    else {
        return Err("日期序列规格须为 YYYY-MM-DD~YYYY-MM-DD[:步长]（或 ..）；列表未改变。".into());
    };
    Ok(DateSequenceConfig {
        from_date: from.trim().to_owned(),
        to_date: to.trim().to_owned(),
        step_days,
        format,
    })
}

pub(super) fn parse_char_blocks_quick_config(
    raw: &str,
    preset_range: Option<(usize, usize, usize)>,
) -> Result<CharacterBlocksConfig, String> {
    if let Some((min_length, max_length, step)) = preset_range {
        let base = if raw.is_empty() {
            "A".to_owned()
        } else {
            raw.to_owned()
        };
        return Ok(CharacterBlocksConfig {
            base,
            min_length,
            max_length,
            step,
        });
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(CharacterBlocksConfig {
            base: "A".into(),
            min_length: 4,
            max_length: 32,
            step: 4,
        });
    }
    let Some((base_part, tail)) = trimmed.split_once(':') else {
        return Err(
            "字符块规格须为 基块:起始~结束[:步长]（如 A:4~32:4，或使用 ..）；列表未改变。".into(),
        );
    };
    let (range_part, step) = match tail.rsplit_once(':') {
        Some((r, step_raw)) => {
            let s = step_raw
                .trim()
                .parse::<usize>()
                .map_err(|_| "字符块步长必须为正整数；列表未改变。".to_owned())?;
            (r.trim(), s)
        }
        None => (tail.trim(), 1),
    };
    let Some((min_raw, max_raw)) = range_part
        .split_once("..")
        .or_else(|| range_part.split_once('~'))
    else {
        return Err(
            "字符块长度范围须为 基块:起始~结束[:步长]（如 A:4~32:4，或使用 ..）；列表未改变。"
                .into(),
        );
    };
    let min_length = min_raw
        .trim()
        .parse::<usize>()
        .map_err(|_| "字符块起始长度必须为非负整数；列表未改变。".to_owned())?;
    let max_length = max_raw
        .trim()
        .parse::<usize>()
        .map_err(|_| "字符块结束长度必须为非负整数；列表未改变。".to_owned())?;
    Ok(CharacterBlocksConfig {
        base: base_part.to_owned(),
        min_length,
        max_length,
        step,
    })
}

pub(super) fn parse_case_variants_quick_config(raw: &str) -> Result<CaseVariantsConfig, String> {
    let identifiers: Vec<String> = raw
        .split([',', '，'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    if identifiers.is_empty() {
        return Err("请先在输入框中填写标识符（多个词可用逗号分隔）；列表未改变。".into());
    }
    Ok(CaseVariantsConfig { identifiers })
}

pub(super) fn resolve_processing_rule_with_input(
    rule: PayloadProcessingRule,
    raw: &str,
) -> Result<(PayloadProcessingRule, bool), String> {
    match rule {
        PayloadProcessingRule::AddPrefix(_) => {
            if raw.is_empty() {
                return Err("请先在输入框中填写要添加的前缀或后缀文本。".into());
            }
            Ok((PayloadProcessingRule::AddPrefix(raw.to_owned()), true))
        }
        PayloadProcessingRule::AddSuffix(_) => {
            if raw.is_empty() {
                return Err("请先在输入框中填写要添加的前缀或后缀文本。".into());
            }
            Ok((PayloadProcessingRule::AddSuffix(raw.to_owned()), true))
        }
        PayloadProcessingRule::Substring { .. } => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok((
                    PayloadProcessingRule::Substring {
                        offset: 0,
                        length: 8,
                    },
                    false,
                ));
            }
            let Some((off_raw, len_raw)) = trimmed.split_once(':') else {
                return Err(
                    "子串截取参数须为 offset:length（如 0:4，留空默认 0:8）；列表未改变。".into(),
                );
            };
            let offset = off_raw
                .trim()
                .parse::<usize>()
                .map_err(|_| "子串截取起始偏移必须为非负整数；列表未改变。".to_owned())?;
            let length = len_raw
                .trim()
                .parse::<usize>()
                .map_err(|_| "子串截取长度必须为非负整数；列表未改变。".to_owned())?;
            Ok((PayloadProcessingRule::Substring { offset, length }, true))
        }
        PayloadProcessingRule::MatchReplace { .. } => {
            let Some((old, new)) = raw.split_once("=>") else {
                return Err("字面替换参数须为 old=>new；列表未改变。".into());
            };
            if old.is_empty() {
                return Err("字面替换的匹配模式不能为空；列表未改变。".into());
            }
            Ok((
                PayloadProcessingRule::MatchReplace {
                    match_pattern: old.to_owned(),
                    replace_with: new.to_owned(),
                },
                true,
            ))
        }
        PayloadProcessingRule::MatchReplaceRegex { .. } => {
            let Some((pat, new)) = raw.split_once("=>") else {
                return Err("正则替换参数须为 regex=>replace；列表未改变。".into());
            };
            if pat.trim().is_empty() {
                return Err("正则替换的正则表达式不能为空；列表未改变。".into());
            }
            Ok((
                PayloadProcessingRule::MatchReplaceRegex {
                    pattern: pat.to_owned(),
                    replace_with: new.to_owned(),
                },
                true,
            ))
        }
        PayloadProcessingRule::AddIfNotMatchRegex { .. } => {
            let Some((pat, suffix)) = raw.split_once("=>") else {
                return Err("不匹配追加参数须为 regex=>suffix；列表未改变。".into());
            };
            if pat.trim().is_empty() {
                return Err("不匹配追加的正则表达式不能为空；列表未改变。".into());
            }
            Ok((
                PayloadProcessingRule::AddIfNotMatchRegex {
                    pattern: pat.to_owned(),
                    addition: suffix.to_owned(),
                },
                true,
            ))
        }
        PayloadProcessingRule::SkipIfMatchesRegex { .. } => {
            if raw.trim().is_empty() {
                return Err("请先在输入框中填写用于跳过匹配的正则表达式；列表未改变。".into());
            }
            Ok((
                PayloadProcessingRule::SkipIfMatchesRegex {
                    pattern: raw.to_owned(),
                },
                true,
            ))
        }
        other => Ok((other, false)),
    }
}

pub(super) fn generate_payloads_by_type_and_input(
    payload_type: IntruderPayloadType,
    raw: &str,
    existing_sets: &[Vec<Vec<u8>>],
    selected: Option<usize>,
) -> Result<Vec<Vec<u8>>, String> {
    let trimmed = raw.trim();
    match payload_type {
        IntruderPayloadType::SimpleList => {
            if trimmed.is_empty() {
                Err("请先在输入框中填写要添加的简单列表条目。".into())
            } else {
                Ok(trimmed
                    .split([',', '\n'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.as_bytes().to_vec())
                    .collect())
            }
        }
        IntruderPayloadType::RuntimeFile => {
            let (file_path, max_lines) = match trimmed.rsplit_once(':') {
                Some((p, n)) if !p.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
                    (p.trim().to_owned(), n.parse::<usize>().ok())
                }
                _ => (trimmed.to_owned(), None),
            };
            generate_runtime_file_payloads(&RuntimeFileConfig {
                file_path,
                skip_blank_lines: true,
                max_lines,
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::CustomIterator => {
            let slots = if trimmed.is_empty() {
                vec![
                    CustomIteratorSlot {
                        prefix: String::new(),
                        items: vec!["admin".into(), "user".into()],
                        separator: "_".into(),
                    },
                    CustomIteratorSlot {
                        prefix: String::new(),
                        items: vec!["1".into(), "2".into()],
                        separator: String::new(),
                    },
                ]
            } else {
                trimmed
                    .split(';')
                    .filter(|part| !part.trim().is_empty())
                    .map(|part| {
                        let (items_raw, separator) = part
                            .split_once(':')
                            .map_or((part.trim(), ""), |(i, sep)| (i.trim(), sep));
                        CustomIteratorSlot {
                            prefix: String::new(),
                            items: items_raw
                                .split(',')
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .collect(),
                            separator: separator.to_owned(),
                        }
                    })
                    .collect()
            };
            generate_custom_iterator_payloads(&CustomIteratorConfig { slots })
                .map_err(|e| e.to_string())
        }
        IntruderPayloadType::CharacterSubstitution => {
            let base_items: Vec<String> = if trimmed.is_empty() {
                vec!["admin".into(), "test".into()]
            } else {
                trimmed
                    .split([',', '，'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            generate_character_substitution_payloads(&CharacterSubstitutionConfig {
                base_items,
                substitutions: Vec::new(),
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::CaseModification => {
            let items: Vec<String> = if trimmed.is_empty() {
                vec!["AdminUser".into()]
            } else {
                trimmed
                    .split([',', '，'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            generate_case_modification_payloads(&CaseModificationConfig {
                items,
                mode: CaseModificationMode::AllModes,
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::RecursiveGrep => {
            let (seeds_raw, delim_raw) = trimmed
                .split_once('|')
                .map_or((trimmed, "token=..&"), |(s, d)| (s.trim(), d.trim()));
            let initial_payloads: Vec<String> = if seeds_raw.is_empty() {
                vec!["seed_0".into()]
            } else {
                seeds_raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            let (start_delimiter, end_delimiter, regex_pattern) =
                if let Some(re) = delim_raw.strip_prefix("re:") {
                    (String::new(), String::new(), Some(re.to_owned()))
                } else if let Some((s, e)) = delim_raw.split_once("..") {
                    (s.to_owned(), e.to_owned(), None)
                } else {
                    ("token=".into(), "&".into(), None)
                };
            generate_recursive_grep_payloads(
                &RecursiveGrepConfig {
                    initial_payloads,
                    start_delimiter,
                    end_delimiter,
                    regex_pattern,
                    stop_if_duplicate: true,
                },
                &[],
            )
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::IllegalUnicode => {
            let input = if trimmed.is_empty() {
                "/".to_owned()
            } else {
                trimmed.to_owned()
            };
            generate_illegal_unicode_payloads(&IllegalUnicodeConfig {
                input,
                overlong_lengths: vec![2, 3, 4],
                hex_encode: false,
                url_encode: true,
                uppercase_hex: true,
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::CharacterBlocks => {
            let cfg = parse_char_blocks_quick_config(raw, None)?;
            generate_character_block_payloads(&cfg).map_err(|e| e.to_string())
        }
        IntruderPayloadType::Numbers => {
            let (from, to, step) = if let Some((r, s)) = trimmed.split_once(':') {
                let (f, t) = r.split_once("..").unwrap_or(("1", "10"));
                (
                    f.trim().parse::<i64>().unwrap_or(1),
                    t.trim().parse::<i64>().unwrap_or(10),
                    s.trim().parse::<i64>().unwrap_or(1).max(1),
                )
            } else if let Some((f, t)) = trimmed.split_once("..") {
                (
                    f.trim().parse::<i64>().unwrap_or(1),
                    t.trim().parse::<i64>().unwrap_or(10),
                    1,
                )
            } else {
                (1, 10, 1)
            };
            generate_numbers_config_payloads(&NumbersGeneratorConfig {
                from,
                to,
                step,
                sequential: true,
                how_many: 10,
                hex: false,
                min_integer_digits: 1,
                max_integer_digits: None,
                min_fraction_digits: 0,
                seed: 1,
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::Dates => {
            let cfg = parse_date_quick_config(raw, DateOutputFormat::IsoDate)?;
            generate_dates_with_unit_payloads(
                &cfg.from_date,
                &cfg.to_date,
                cfg.step_days,
                DateStepUnit::Day,
                cfg.format,
                None,
            )
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::BruteForcer => {
            let (charset, min_length, max_length) = if let Some((cs, r)) = trimmed.split_once(':') {
                let (mn, mx) = r.split_once("..").unwrap_or(("1", "2"));
                (
                    if cs.is_empty() { "abc" } else { cs }.to_owned(),
                    mn.trim().parse::<usize>().unwrap_or(1),
                    mx.trim().parse::<usize>().unwrap_or(2),
                )
            } else if !trimmed.is_empty() {
                (trimmed.to_owned(), 1, 2)
            } else {
                ("abc".to_owned(), 1, 2)
            };
            generate_brute_forcer_payloads(&BruteForcerConfig {
                charset,
                min_length,
                max_length,
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::NullPayloads => {
            let cfg = parse_empty_repeat_quick_config(raw)?;
            generate_empty_repeats(&cfg).map_err(|e| e.to_string())
        }
        IntruderPayloadType::CharacterFrobber => {
            let base_value = if trimmed.is_empty() {
                "admin".to_owned()
            } else {
                trimmed.to_owned()
            };
            generate_character_frobber_payloads(&CharacterFrobberConfig { base_value })
                .map_err(|e| e.to_string())
        }
        IntruderPayloadType::BitFlipper => {
            let (base_value, format) = if let Some(hex_val) = trimmed.strip_prefix("hex:") {
                (hex_val.trim().to_owned(), BitFlipperFormat::AsciiHex)
            } else if trimmed.is_empty() {
                ("ab".to_owned(), BitFlipperFormat::LiteralBytes)
            } else {
                (trimmed.to_owned(), BitFlipperFormat::LiteralBytes)
            };
            generate_bit_flipper_payloads(&BitFlipperConfig {
                base_value,
                format,
                bit_mask: 0xff,
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::UsernameGenerator => {
            let names: Vec<String> = if trimmed.is_empty() {
                vec!["John Smith".into()]
            } else {
                trimmed
                    .split([',', '，'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            generate_username_payloads(&UsernameGeneratorConfig {
                names,
                max_per_name: Some(32),
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::EcbBlockShuffler => {
            let (base_ciphertext, block_size) = if let Some((ct, bs)) = trimmed.split_once(':') {
                (
                    ct.trim().to_owned(),
                    bs.trim().parse::<usize>().unwrap_or(8),
                )
            } else if trimmed.is_empty() {
                ("00112233445566778899aabbccddeeff".to_owned(), 8)
            } else {
                (trimmed.to_owned(), 8)
            };
            generate_ecb_block_shuffler_payloads(&EcbBlockShufflerConfig {
                base_ciphertext,
                is_hex: true,
                block_size,
                additional_encrypted_strings: Vec::new(),
            })
            .map_err(|e| e.to_string())
        }
        IntruderPayloadType::CopyOtherPayload => {
            let source_one_based = trimmed.parse::<usize>().unwrap_or(1).max(1);
            let mut source_idx = source_one_based - 1;
            if selected == Some(source_idx) && existing_sets.len() > 1 {
                source_idx = usize::from(source_idx == 0);
            }
            let source = existing_sets
                .get(source_idx)
                .ok_or_else(|| format!("源 Payload 集 {} 不存在；列表未改变。", source_idx + 1))?;
            if source.is_empty() {
                return Err(format!(
                    "源 Payload 集 {} 为空；列表未改变。",
                    source_idx + 1
                ));
            }
            Ok(source.clone())
        }
        IntruderPayloadType::ExtensionGenerated => {
            Err("当前未加载任何可用的 payload_generator 扩展插件；列表未改变。".into())
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn generate_for_intruder_payload_type(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let payload_type = self.current_intruder_payload_type(id, editors);
        if payload_type == IntruderPayloadType::ExtensionGenerated
            && let Some((plugin_uuid, _)) = self.live_payload_plugins("payload_generator").first() {
                let uuid = *plugin_uuid;
                self.generate_plugin_intruder_payloads(id, uuid, window, cx);
                return;
            }
        let selected = editors.payload_position;
        let input = editors.payload_input.clone();
        let raw = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let existing_sets = self
            .workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.spec.payload_sets.clone())
            .unwrap_or_default();
        let had_input = !raw.trim().is_empty();
        match generate_payloads_by_type_and_input(payload_type, &raw, &existing_sets, selected) {
            Ok(values) => {
                if self.apply_processed_intruder_payloads(id, selected, values, window, cx)
                    && had_input
                {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn generate_intruder_dates(
        &mut self,
        id: TabId,
        format: DateOutputFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let input = editors.payload_input.clone();
        let raw = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let had_input = !raw.trim().is_empty();
        let config = match parse_date_quick_config(&raw, format) {
            Ok(cfg) => cfg,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        match generate_date_sequence_payloads(&config) {
            Ok(values) => {
                if self.append_intruder_payloads(id, &values, window, cx) && had_input {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
            }
            Err(error) => self.fail(error.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn generate_intruder_char_blocks(
        &mut self,
        id: TabId,
        preset_range: Option<(usize, usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let input = editors.payload_input.clone();
        let raw = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let had_input = !raw.is_empty();
        let config = match parse_char_blocks_quick_config(&raw, preset_range) {
            Ok(cfg) => cfg,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        match generate_character_block_payloads(&config) {
            Ok(values) => {
                if self.append_intruder_payloads(id, &values, window, cx) && had_input {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
            }
            Err(error) => self.fail(error.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn generate_intruder_case_variants(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let input = editors.payload_input.clone();
        let raw = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let config = match parse_case_variants_quick_config(&raw) {
            Ok(cfg) => cfg,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        match generate_case_variant_payloads(&config) {
            Ok(values) => {
                if self.append_intruder_payloads(id, &values, window, cx) {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
            }
            Err(error) => self.fail(error.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn generate_intruder_empty_repeats(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let input = editors.payload_input.clone();
        let raw = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let had_input = !raw.trim().is_empty();
        let config = match parse_empty_repeat_quick_config(&raw) {
            Ok(cfg) => cfg,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        match generate_empty_repeats(&config) {
            Ok(values) => {
                if self.append_intruder_payloads(id, &values, window, cx) && had_input {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
            }
            Err(error) => self.fail(error.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn copy_first_intruder_payload_set(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self
            .intruder
            .editors
            .get(&id)
            .and_then(|editors| editors.payload_position)
            .filter(|&index| index > 0)
        else {
            self.fail(
                "请先切换到第 2 个或之后的 Payload 位置，再复制第 1 组列表。",
                cx,
            );
            return;
        };
        let Some(source) = self
            .workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .and_then(|tab| tab.spec.payload_sets.first())
            .cloned()
        else {
            self.fail("第 1 组 payload 列表尚未初始化；列表未改变。", cx);
            return;
        };
        if self.try_change_intruder_spec(
            id,
            |spec| {
                let sets = expected_payload_sets(spec).max(spec.payload_sets.len());
                if sets > MAX_PAYLOAD_SETS || selected >= sets {
                    return Err("选中的 payload 位置已不存在；列表未改变。".into());
                }
                let mut next_sets = spec.payload_sets.clone();
                next_sets.resize_with(sets, Vec::new);
                next_sets[selected] = source;
                let total_count = next_sets
                    .iter()
                    .try_fold(0usize, |sum, set| sum.checked_add(set.len()))
                    .ok_or_else(|| "Payload 总数量溢出；列表未改变。".to_owned())?;
                if total_count > MAX_PAYLOADS {
                    return Err("复制后 payload 总数量将超过 100000；列表未改变。".into());
                }
                let total_bytes = next_sets
                    .iter()
                    .flatten()
                    .try_fold(0usize, |sum, value| sum.checked_add(value.len()))
                    .ok_or_else(|| "Payload 总字节数溢出；列表未改变。".to_owned())?;
                if total_bytes > MAX_PAYLOAD_BYTES {
                    return Err("复制后 payload 总字节数将超过 32 MiB；列表未改变。".into());
                }
                spec.payload_sets = next_sets;
                Ok(())
            },
            window,
            cx,
        ) && let Some(editors) = self.intruder.editors.get_mut(&id)
        {
            editors.payload_selection.clear();
        }
    }
}
