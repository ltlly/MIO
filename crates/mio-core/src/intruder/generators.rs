use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike as _, NaiveDate, TimeDelta};
use serde::{Deserialize, Serialize};

use super::{MAX_PAYLOAD_BYTES, MAX_PAYLOADS};
use crate::codec_util;

pub const MAX_DATE_SEQUENCE_ITEMS: usize = 10_000;
pub const MAX_DATE_STEP_DAYS: u16 = 366;
pub const MAX_CHAR_BLOCK_ITEMS: usize = 10_000;
pub const MAX_CHAR_BLOCK_BYTES: usize = 64 * 1024;
pub const MAX_CASE_VARIANTS: usize = 1_024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntruderPayloadType {
    #[default]
    SimpleList,
    RuntimeFile,
    CustomIterator,
    CharacterSubstitution,
    CaseModification,
    RecursiveGrep,
    IllegalUnicode,
    CharacterBlocks,
    Numbers,
    Dates,
    BruteForcer,
    NullPayloads,
    CharacterFrobber,
    BitFlipper,
    UsernameGenerator,
    EcbBlockShuffler,
    CopyOtherPayload,
    ExtensionGenerated,
}

impl IntruderPayloadType {
    pub const ALL: [Self; 18] = [
        Self::SimpleList,
        Self::RuntimeFile,
        Self::CustomIterator,
        Self::CharacterSubstitution,
        Self::CaseModification,
        Self::RecursiveGrep,
        Self::IllegalUnicode,
        Self::CharacterBlocks,
        Self::Numbers,
        Self::Dates,
        Self::BruteForcer,
        Self::NullPayloads,
        Self::CharacterFrobber,
        Self::BitFlipper,
        Self::UsernameGenerator,
        Self::EcbBlockShuffler,
        Self::CopyOtherPayload,
        Self::ExtensionGenerated,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SimpleList => "Simple list",
            Self::RuntimeFile => "Runtime file",
            Self::CustomIterator => "Custom iterator",
            Self::CharacterSubstitution => "Character substitution",
            Self::CaseModification => "Case modification",
            Self::RecursiveGrep => "Recursive grep",
            Self::IllegalUnicode => "Illegal Unicode",
            Self::CharacterBlocks => "Character blocks",
            Self::Numbers => "Numbers",
            Self::Dates => "Dates",
            Self::BruteForcer => "Brute forcer",
            Self::NullPayloads => "Null payloads",
            Self::CharacterFrobber => "Character frobber",
            Self::BitFlipper => "Bit flipper",
            Self::UsernameGenerator => "Username generator",
            Self::EcbBlockShuffler => "ECB block shuffler",
            Self::CopyOtherPayload => "Copy other payload",
            Self::ExtensionGenerated => "Extension-generated",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::SimpleList => "配置作为 payload 使用的简单字符串列表。",
            Self::RuntimeFile => {
                "从指定字典文件按行流式读取 payload，可设置跳过空行与读取条数上限。"
            }
            Self::CustomIterator => "通过多个槽位的词表、前缀与分隔符笛卡尔积拼接生成 payload。",
            Self::CharacterSubstitution => {
                "对基础词表执行字符替换（如 e->3, a->4, o->0, i->1, s->5）的全部组合变体。"
            }
            Self::CaseModification => {
                "生成原值、全小写、全大写、To Propername 与 To properName 大小写变形。"
            }
            Self::RecursiveGrep => {
                "从响应中使用起止分隔符或正则提取字段，作为下一轮请求的 payload 与初始种子。"
            }
            Self::IllegalUnicode => {
                "生成目标字符的超长 UTF-8 非法编码字节序列，支持 Hex 与 URL 编码。"
            }
            Self::CharacterBlocks => "按指定基础字符串与最小/最大长度、步长生成字符块 payload。",
            Self::Numbers => {
                "支持顺序/随机、From/To/Step、十进制/十六进制与整数/小数位数的数字序列。"
            }
            Self::Dates => "支持自定义格式与按日、周、月、年步进的日期序列。",
            Self::BruteForcer => "按指定字符集与最小/最大长度，按字典序穷举生成 payload。",
            Self::NullPayloads => "生成指定数量的空负载，用于重放、时序或条件竞争测试。",
            Self::CharacterFrobber => {
                "对原字符串或基础字符串的每个字符位置依次递增生成变形 payload。"
            }
            Self::BitFlipper => {
                "对原值或基础值按指定掩码逐比特翻转，支持原字节或 ASCII 十六进制格式。"
            }
            Self::UsernameGenerator => {
                "对输入姓名自动生成 john.smith、jsmith、smithj、john_smith 等全部标准用户名变体。"
            }
            Self::EcbBlockShuffler => {
                "按 8 或 16 字节块大小对十六进制或原始密文块执行交换、复制、移除与插入洗牌。"
            }
            Self::CopyOtherPayload => "复制指定源 payload 集的完整内容到当前 payload 集。",
            Self::ExtensionGenerated => "绑定已加载插件的 payload 生成器并生成 payload 序列。",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeFileConfig {
    pub file_path: String,
    #[serde(default = "default_true")]
    pub skip_blank_lines: bool,
    #[serde(default)]
    pub max_lines: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomIteratorSlot {
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub separator: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomIteratorConfig {
    #[serde(default)]
    pub slots: Vec<CustomIteratorSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterSubstitutionConfig {
    #[serde(default)]
    pub base_items: Vec<String>,
    #[serde(default)]
    pub substitutions: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseModificationMode {
    NoChange,
    ToLowerCase,
    ToUpperCase,
    ToProperName,
    ToProperCamelName,
    #[default]
    AllModes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseModificationConfig {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub mode: CaseModificationMode,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecursiveGrepConfig {
    #[serde(default)]
    pub initial_payloads: Vec<String>,
    #[serde(default)]
    pub start_delimiter: String,
    #[serde(default)]
    pub end_delimiter: String,
    #[serde(default)]
    pub regex_pattern: Option<String>,
    #[serde(default = "default_true")]
    pub stop_if_duplicate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IllegalUnicodeConfig {
    #[serde(default = "default_illegal_unicode_input")]
    pub input: String,
    #[serde(default = "default_illegal_unicode_lengths")]
    pub overlong_lengths: Vec<usize>,
    #[serde(default)]
    pub hex_encode: bool,
    #[serde(default)]
    pub url_encode: bool,
    #[serde(default)]
    pub uppercase_hex: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumbersGeneratorConfig {
    pub from: i64,
    pub to: i64,
    pub step: i64,
    #[serde(default = "default_true")]
    pub sequential: bool,
    #[serde(default = "default_numbers_count")]
    pub how_many: usize,
    #[serde(default)]
    pub hex: bool,
    #[serde(default = "default_one_usize")]
    pub min_integer_digits: usize,
    #[serde(default)]
    pub max_integer_digits: Option<usize>,
    #[serde(default)]
    pub min_fraction_digits: usize,
    #[serde(default)]
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateStepUnit {
    #[default]
    Day,
    Week,
    Month,
    Year,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BruteForcerConfig {
    #[serde(default = "default_brute_forcer_charset")]
    pub charset: String,
    #[serde(default = "default_one_usize")]
    pub min_length: usize,
    #[serde(default = "default_brute_forcer_max_length")]
    pub max_length: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterFrobberConfig {
    #[serde(default)]
    pub base_value: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitFlipperFormat {
    #[default]
    LiteralBytes,
    AsciiHex,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitFlipperConfig {
    #[serde(default = "default_bit_flipper_base")]
    pub base_value: String,
    #[serde(default)]
    pub format: BitFlipperFormat,
    #[serde(default = "default_bit_flipper_mask")]
    pub bit_mask: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsernameGeneratorConfig {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub max_per_name: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcbBlockShufflerConfig {
    #[serde(default = "default_ecb_base_ciphertext")]
    pub base_ciphertext: String,
    #[serde(default = "default_true")]
    pub is_hex: bool,
    #[serde(default = "default_ecb_block_size")]
    pub block_size: usize,
    #[serde(default)]
    pub additional_encrypted_strings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyOtherPayloadConfig {
    #[serde(default = "default_one_usize")]
    pub source_set_index: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionGeneratedConfig {
    #[serde(default)]
    pub generator_name: String,
}

const fn default_true() -> bool {
    true
}

const fn default_one_usize() -> usize {
    1
}

const fn default_numbers_count() -> usize {
    100
}

const fn default_brute_forcer_max_length() -> usize {
    3
}

const fn default_ecb_block_size() -> usize {
    16
}

fn default_illegal_unicode_input() -> String {
    "/".into()
}

fn default_illegal_unicode_lengths() -> Vec<usize> {
    vec![2, 3, 4, 5, 6]
}

fn default_brute_forcer_charset() -> String {
    "abcdefghijklmnopqrstuvwxyz0123456789".into()
}

fn default_bit_flipper_base() -> String {
    "abc".into()
}

const fn default_bit_flipper_mask() -> u8 {
    0xff
}

fn default_ecb_base_ciphertext() -> String {
    "00112233445566778899aabbccddeeff".into()
}

fn ensure_generator_limits(count: usize, bytes: usize, name: &str) -> Result<()> {
    ensure!(
        count <= MAX_PAYLOADS,
        "{name} 生成数量超过 {MAX_PAYLOADS} 上限"
    );
    ensure!(
        bytes <= MAX_PAYLOAD_BYTES,
        "{name} 生成总字节数超过 {MAX_PAYLOAD_BYTES} 限额"
    );
    Ok(())
}

fn push_generated_payload(
    out: &mut Vec<Vec<u8>>,
    seen: &mut std::collections::HashSet<Vec<u8>>,
    total_bytes: &mut usize,
    value: Vec<u8>,
    name: &str,
) -> Result<()> {
    if !seen.insert(value.clone()) {
        return Ok(());
    }
    let next_count = out.len().checked_add(1).context("payload 数量溢出")?;
    let next_bytes = total_bytes
        .checked_add(value.len())
        .context("payload 总字节数溢出")?;
    ensure_generator_limits(next_count, next_bytes, name)?;
    *total_bytes = next_bytes;
    out.push(value);
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateOutputFormat {
    #[default]
    IsoDate,
    Compact,
    Slash,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateSequenceConfig {
    pub from_date: String,
    pub to_date: String,
    pub step_days: u16,
    pub format: DateOutputFormat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterBlocksConfig {
    pub base: String,
    pub min_length: usize,
    pub max_length: usize,
    pub step: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseVariantsConfig {
    pub identifiers: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyRepeatConfig {
    pub count: usize,
}

pub fn generate_empty_repeats(config: &EmptyRepeatConfig) -> Result<Vec<Vec<u8>>> {
    ensure!(
        (1..=MAX_PAYLOADS).contains(&config.count),
        "重复基准次数必须在 1..={MAX_PAYLOADS} 之间"
    );
    Ok(vec![Vec::new(); config.count])
}

fn days_in_month_chrono(year: i32, month: u32) -> u32 {
    let (next_y, next_m) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_y, next_m, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

fn parse_iso_date(raw: &str) -> Result<NaiveDate> {
    let trimmed = raw.trim();
    let parts: Vec<&str> = trimmed.split('-').collect();
    ensure!(
        parts.len() == 3 && parts[0].len() == 4 && parts[1].len() == 2 && parts[2].len() == 2,
        "日期须为 YYYY-MM-DD 格式：{trimmed}"
    );
    ensure!(
        parts
            .iter()
            .all(|part| part.bytes().all(|b| b.is_ascii_digit())),
        "日期仅允许数字：{trimmed}"
    );
    let year: i32 = parts[0].parse().context("年份无效")?;
    let month: u32 = parts[1].parse().context("月份无效")?;
    let day: u32 = parts[2].parse().context("日期无效")?;
    ensure!((1..=9999).contains(&year), "年份须在 0001..=9999 之间");
    ensure!((1..=12).contains(&month), "月份须在 01..=12 之间");
    NaiveDate::from_ymd_opt(year, month, day)
        .with_context(|| format!("日期超出当月最大天数：{trimmed}"))
}

fn format_ymd(year: i32, month: u32, day: u32, format: DateOutputFormat) -> String {
    match format {
        DateOutputFormat::IsoDate => format!("{year:04}-{month:02}-{day:02}"),
        DateOutputFormat::Compact => format!("{year:04}{month:02}{day:02}"),
        DateOutputFormat::Slash => format!("{year:04}/{month:02}/{day:02}"),
    }
}

pub fn generate_date_sequence_payloads(config: &DateSequenceConfig) -> Result<Vec<Vec<u8>>> {
    ensure!(
        (1..=MAX_DATE_STEP_DAYS).contains(&config.step_days),
        "日期步长天数必须在 1..={MAX_DATE_STEP_DAYS} 之间"
    );
    let start = parse_iso_date(&config.from_date)?;
    let end = parse_iso_date(&config.to_date)?;
    let step = i64::from(config.step_days);
    let span = (end - start).num_days().unsigned_abs();
    let count_u64 = span / (step as u64) + 1;
    ensure!(
        count_u64 <= MAX_DATE_SEQUENCE_ITEMS as u64,
        "生成日期数量超过 {MAX_DATE_SEQUENCE_ITEMS} 上限"
    );
    let count = count_u64 as usize;
    let direction = if start <= end { 1_i64 } else { -1_i64 };
    let mut out = Vec::with_capacity(count);
    for idx in 0..count {
        let cur = start + TimeDelta::days((idx as i64) * step * direction);
        out.push(format_ymd(cur.year(), cur.month(), cur.day(), config.format).into_bytes());
    }
    Ok(out)
}

pub fn generate_character_block_payloads(config: &CharacterBlocksConfig) -> Result<Vec<Vec<u8>>> {
    let base_bytes = config.base.as_bytes();
    ensure!(!base_bytes.is_empty(), "字符块基础字符串不能为空");
    ensure!(config.step > 0, "字符块步长必须为正整数");
    ensure!(
        config.min_length <= MAX_CHAR_BLOCK_BYTES && config.max_length <= MAX_CHAR_BLOCK_BYTES,
        "字符块单条长度不能超过 {MAX_CHAR_BLOCK_BYTES} 字节（64 KiB）"
    );
    let span = config.max_length.abs_diff(config.min_length);
    let count = span
        .checked_div(config.step)
        .and_then(|steps| steps.checked_add(1))
        .context("字符块条目数量溢出")?;
    ensure!(
        count <= MAX_CHAR_BLOCK_ITEMS,
        "字符块生成总数超过 {MAX_CHAR_BLOCK_ITEMS} 条上限"
    );
    let mut total_bytes = 0usize;
    let mut lengths = Vec::with_capacity(count);
    for idx in 0..count {
        let delta = idx.checked_mul(config.step).context("字符块长度计算溢出")?;
        let len = if config.min_length <= config.max_length {
            config.min_length + delta
        } else {
            config.min_length - delta
        };
        total_bytes = total_bytes.checked_add(len).context("字符块总字节数溢出")?;
        ensure!(
            total_bytes <= MAX_PAYLOAD_BYTES,
            "字符块总字节数超过 {MAX_PAYLOAD_BYTES} 限额"
        );
        lengths.push(len);
    }
    let mut out = Vec::with_capacity(count);
    for len in lengths {
        let mut buf = Vec::with_capacity(len);
        while buf.len() + base_bytes.len() <= len {
            buf.extend_from_slice(base_bytes);
        }
        let rem = len - buf.len();
        if rem > 0 {
            buf.extend_from_slice(&base_bytes[..rem]);
        }
        out.push(buf);
    }
    Ok(out)
}

fn split_identifier_words(raw: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = raw.chars().collect();
    for (idx, &ch) in chars.iter().enumerate() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = idx.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(idx + 1).copied();
        let boundary = ch.is_uppercase()
            && !cur.is_empty()
            && (prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit())
                || (prev.is_some_and(char::is_uppercase) && next.is_some_and(char::is_lowercase)));
        if boundary {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn capitalize_word(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = first.to_uppercase().collect::<String>();
    out.push_str(&chars.as_str().to_lowercase());
    out
}

pub fn generate_case_variant_payloads(config: &CaseVariantsConfig) -> Result<Vec<Vec<u8>>> {
    if config.identifiers.is_empty() {
        bail!("标识符列表不能为空");
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut total_bytes = 0usize;
    for raw in &config.identifiers {
        let ident = raw.trim();
        if ident.is_empty() {
            continue;
        }
        ensure!(
            ident.len() <= MAX_CHAR_BLOCK_BYTES,
            "单个标识符长度不能超过 {MAX_CHAR_BLOCK_BYTES} 字节"
        );
        let words = split_identifier_words(ident);
        let mut candidates = vec![ident.to_lowercase(), ident.to_uppercase()];
        if !words.is_empty() {
            let pascal: String = words.iter().map(|w| capitalize_word(w)).collect();
            let mut camel = words[0].to_lowercase();
            for w in &words[1..] {
                camel.push_str(&capitalize_word(w));
            }
            let snake = words
                .iter()
                .map(|w| w.to_lowercase())
                .collect::<Vec<_>>()
                .join("_");
            let screaming = snake.to_uppercase();
            let kebab = words
                .iter()
                .map(|w| w.to_lowercase())
                .collect::<Vec<_>>()
                .join("-");
            candidates.extend([pascal, camel, snake, screaming, kebab]);
        }
        for candidate in candidates {
            if seen.insert(candidate.clone()) {
                ensure!(
                    out.len() < MAX_CASE_VARIANTS,
                    "大小写变体数量超过 {MAX_CASE_VARIANTS} 条上限"
                );
                total_bytes = total_bytes
                    .checked_add(candidate.len())
                    .context("变体总字节数溢出")?;
                ensure!(
                    total_bytes <= MAX_PAYLOAD_BYTES,
                    "变体总字节数超过 {MAX_PAYLOAD_BYTES} 限额"
                );
                out.push(candidate.into_bytes());
            }
        }
    }
    ensure!(!out.is_empty(), "没有可生成变体的非空标识符");
    Ok(out)
}

pub fn generate_runtime_file_payloads(config: &RuntimeFileConfig) -> Result<Vec<Vec<u8>>> {
    let path_str = config.file_path.trim();
    ensure!(!path_str.is_empty(), "运行时文件路径不能为空");
    let raw = std::fs::read(path_str).with_context(|| format!("读取运行时文件失败：{path_str}"))?;
    ensure!(
        raw.len() <= MAX_PAYLOAD_BYTES,
        "运行时文件超过 {MAX_PAYLOAD_BYTES} 字节限额"
    );
    let max_lines = config.max_lines.unwrap_or(MAX_PAYLOADS).min(MAX_PAYLOADS);
    let mut out = Vec::new();
    for line in raw.split_inclusive(|b| *b == b'\n') {
        if out.len() >= max_lines {
            break;
        }
        let trimmed = line
            .strip_suffix(b"\n")
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .unwrap_or(line);
        if config.skip_blank_lines && trimmed.is_empty() {
            continue;
        }
        out.push(trimmed.to_vec());
    }
    ensure!(!out.is_empty(), "运行时文件未读取到任何非空 payload");
    Ok(out)
}

pub fn generate_custom_iterator_payloads(config: &CustomIteratorConfig) -> Result<Vec<Vec<u8>>> {
    let active_slots: Vec<&CustomIteratorSlot> = config
        .slots
        .iter()
        .filter(|s| !s.items.is_empty() || !s.prefix.is_empty())
        .collect();
    ensure!(
        !active_slots.is_empty(),
        "Custom iterator 至少需要一个非空槽位"
    );
    ensure!(active_slots.len() <= 8, "Custom iterator 最多支持 8 个槽位");
    let mut current: Vec<Vec<u8>> = vec![Vec::new()];
    for (slot_idx, slot) in active_slots.iter().enumerate() {
        let items: Vec<&str> = if slot.items.is_empty() {
            vec![""]
        } else {
            slot.items.iter().map(String::as_str).collect()
        };
        let next_count = current
            .len()
            .checked_mul(items.len())
            .context("Custom iterator 组合数量溢出")?;
        ensure!(
            next_count <= MAX_PAYLOADS,
            "Custom iterator 组合数量超过 {MAX_PAYLOADS} 上限"
        );
        let mut next = Vec::with_capacity(next_count);
        let mut total_bytes = 0usize;
        let sep = if slot_idx + 1 < active_slots.len() {
            slot.separator.as_bytes()
        } else {
            b""
        };
        for prefix in &current {
            for item in &items {
                let mut combined =
                    Vec::with_capacity(prefix.len() + slot.prefix.len() + item.len() + sep.len());
                combined.extend_from_slice(prefix);
                combined.extend_from_slice(slot.prefix.as_bytes());
                combined.extend_from_slice(item.as_bytes());
                combined.extend_from_slice(sep);
                total_bytes = total_bytes
                    .checked_add(combined.len())
                    .context("Custom iterator 总字节数溢出")?;
                ensure_generator_limits(next.len() + 1, total_bytes, "Custom iterator")?;
                next.push(combined);
            }
        }
        current = next;
    }
    Ok(current)
}

pub fn generate_character_substitution_payloads(
    config: &CharacterSubstitutionConfig,
) -> Result<Vec<Vec<u8>>> {
    ensure!(!config.base_items.is_empty(), "字符替换基础词表不能为空");
    let rules: Vec<(char, &str)> = if config.substitutions.is_empty() {
        vec![
            ('e', "3"),
            ('a', "4"),
            ('o', "0"),
            ('i', "1"),
            ('s', "5"),
            ('t', "7"),
        ]
    } else {
        let mut parsed = Vec::new();
        for (from, to) in &config.substitutions {
            if let Some(ch) = from.chars().next() {
                parsed.push((ch, to.as_str()));
            }
        }
        parsed
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    for item in &config.base_items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let chars: Vec<char> = trimmed.chars().collect();
        let mut match_positions = Vec::new();
        for (pos, &ch) in chars.iter().enumerate() {
            for &(target, rep) in &rules {
                if ch.eq_ignore_ascii_case(&target) {
                    match_positions.push((pos, rep));
                    break;
                }
            }
            if match_positions.len() >= 10 {
                break;
            }
        }
        let combinations = 1usize << match_positions.len();
        for mask in 0..combinations {
            let mut buf = String::with_capacity(trimmed.len() + 8);
            let mut m_idx = 0usize;
            for (pos, &ch) in chars.iter().enumerate() {
                if m_idx < match_positions.len() && match_positions[m_idx].0 == pos {
                    if (mask & (1 << m_idx)) != 0 {
                        buf.push_str(match_positions[m_idx].1);
                    } else {
                        buf.push(ch);
                    }
                    m_idx += 1;
                } else {
                    buf.push(ch);
                }
            }
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                buf.into_bytes(),
                "Character substitution",
            )?;
        }
    }
    ensure!(!out.is_empty(), "没有可生成字符替换变体的基础词条");
    Ok(out)
}

pub fn generate_case_modification_payloads(
    config: &CaseModificationConfig,
) -> Result<Vec<Vec<u8>>> {
    ensure!(
        !config.items.is_empty(),
        "Case modification 输入词表不能为空"
    );
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    let apply_mode = |raw: &str, mode: CaseModificationMode| -> String {
        match mode {
            CaseModificationMode::NoChange => raw.to_owned(),
            CaseModificationMode::ToLowerCase => raw.to_lowercase(),
            CaseModificationMode::ToUpperCase => raw.to_uppercase(),
            CaseModificationMode::ToProperName => capitalize_word(raw),
            CaseModificationMode::ToProperCamelName => {
                let mut chars = raw.chars();
                match chars.next() {
                    Some(first) => {
                        let mut s = first.to_lowercase().collect::<String>();
                        s.push_str(&chars.as_str().to_uppercase());
                        s
                    }
                    None => String::new(),
                }
            }
            CaseModificationMode::AllModes => raw.to_owned(),
        }
    };
    let modes: &[CaseModificationMode] = match &config.mode {
        CaseModificationMode::AllModes => &[
            CaseModificationMode::NoChange,
            CaseModificationMode::ToLowerCase,
            CaseModificationMode::ToUpperCase,
            CaseModificationMode::ToProperName,
            CaseModificationMode::ToProperCamelName,
        ],
        single => std::slice::from_ref(single),
    };
    for item in &config.items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        for &mode in modes {
            let transformed = apply_mode(trimmed, mode);
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                transformed.into_bytes(),
                "Case modification",
            )?;
        }
    }
    ensure!(!out.is_empty(), "Case modification 未生成任何非空条目");
    Ok(out)
}

pub fn generate_recursive_grep_payloads(
    config: &RecursiveGrepConfig,
    responses: &[&[u8]],
) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    for seed in &config.initial_payloads {
        let trimmed = seed.trim();
        if !trimmed.is_empty() {
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                trimmed.as_bytes().to_vec(),
                "Recursive grep",
            )?;
        }
    }
    let compiled_re = if let Some(pat) = config.regex_pattern.as_deref().filter(|p| !p.is_empty()) {
        Some(
            regex::bytes::RegexBuilder::new(pat)
                .size_limit(256 * 1024)
                .dfa_size_limit(256 * 1024)
                .build()
                .context("Recursive grep 正则表达式无效")?,
        )
    } else {
        None
    };
    for &resp in responses {
        if let Some(re) = &compiled_re {
            for caps in re.captures_iter(resp) {
                let m = caps.get(1).or_else(|| caps.get(0));
                if let Some(m) = m {
                    let candidate = m.as_bytes().to_vec();
                    if config.stop_if_duplicate && seen.contains(&candidate) {
                        continue;
                    }
                    push_generated_payload(&mut out, &mut seen, &mut total_bytes, candidate, "Recursive grep")?;
                }
            }
        } else if !config.start_delimiter.is_empty() && !config.end_delimiter.is_empty() {
            let sb = config.start_delimiter.as_bytes();
            let eb = config.end_delimiter.as_bytes();
            let mut rest = resp;
            while let Some(s_pos) = rest.windows(sb.len()).position(|w| w == sb) {
                let after_s = &rest[s_pos + sb.len()..];
                let Some(e_pos) = after_s.windows(eb.len()).position(|w| w == eb) else {
                    break;
                };
                let candidate = after_s[..e_pos].to_vec();
                rest = &after_s[e_pos + eb.len()..];
                if !candidate.is_empty() {
                    if config.stop_if_duplicate && seen.contains(&candidate) {
                        continue;
                    }
                    push_generated_payload(&mut out, &mut seen, &mut total_bytes, candidate, "Recursive grep")?;
                }
            }
        }
    }
    ensure!(!out.is_empty(), "Recursive grep 初始种子与提取结果均为空");
    Ok(out)
}

fn encode_overlong_utf8(codepoint: u32, length: usize) -> Option<Vec<u8>> {
    if !(2..=6).contains(&length) {
        return None;
    }
    let mut out = vec![0u8; length];
    let mut rem = codepoint;
    for idx in (1..length).rev() {
        out[idx] = 0x80 | ((rem & 0x3f) as u8);
        rem >>= 6;
    }
    let prefix_bits = match length {
        2 => (0xc0u8, 0x1fu32),
        3 => (0xe0u8, 0x0fu32),
        4 => (0xf0u8, 0x07u32),
        5 => (0xf8u8, 0x03u32),
        6 => (0xfcu8, 0x01u32),
        _ => return None,
    };
    if rem > prefix_bits.1 {
        return None;
    }
    out[0] = prefix_bits.0 | (rem as u8);
    Some(out)
}

pub fn generate_illegal_unicode_payloads(config: &IllegalUnicodeConfig) -> Result<Vec<Vec<u8>>> {
    ensure!(
        !config.input.is_empty(),
        "Illegal Unicode 目标字符或文本不能为空"
    );
    let lengths = if config.overlong_lengths.is_empty() {
        vec![2, 3, 4, 5, 6]
    } else {
        config.overlong_lengths.clone()
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    let chars: Vec<char> = config.input.chars().collect();
    for (char_idx, &ch) in chars.iter().enumerate() {
        let cp = ch as u32;
        for &len in &lengths {
            let Some(overlong) = encode_overlong_utf8(cp, len) else {
                continue;
            };
            let encoded_seq: Vec<u8> = if config.url_encode {
                let mut s = String::with_capacity(overlong.len() * 3);
                for b in &overlong {
                    if config.uppercase_hex {
                        s.push_str(&format!("%{b:02X}"));
                    } else {
                        s.push_str(&format!("%{b:02x}"));
                    }
                }
                s.into_bytes()
            } else if config.hex_encode {
                let mut s = String::with_capacity(overlong.len() * 2);
                for b in &overlong {
                    if config.uppercase_hex {
                        s.push_str(&format!("{b:02X}"));
                    } else {
                        s.push_str(&format!("{b:02x}"));
                    }
                }
                s.into_bytes()
            } else {
                overlong
            };
            let mut full = Vec::new();
            for (idx, &other) in chars.iter().enumerate() {
                if idx == char_idx {
                    full.extend_from_slice(&encoded_seq);
                } else {
                    let mut buf = [0u8; 4];
                    full.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
            }
            push_generated_payload(&mut out, &mut seen, &mut total_bytes, full, "Illegal Unicode")?;
        }
    }
    ensure!(!out.is_empty(), "Illegal Unicode 未生成任何有效超长序列");
    Ok(out)
}

pub fn generate_numbers_config_payloads(config: &NumbersGeneratorConfig) -> Result<Vec<Vec<u8>>> {
    ensure!(
        (1..=32).contains(&config.min_integer_digits),
        "最少整数位数必须在 1..=32 之间"
    );
    if let Some(max_d) = config.max_integer_digits {
        ensure!(
            max_d >= config.min_integer_digits && max_d <= 32,
            "最大整数位数必须 >= 最少整数位数且 <= 32"
        );
    }
    ensure!(config.min_fraction_digits <= 16, "最少小数位数不能超过 16");
    let format_num = |value: i128| -> Vec<u8> {
        let magnitude = value.unsigned_abs();
        let mut int_part = if config.hex {
            format!("{magnitude:0width$X}", width = config.min_integer_digits)
        } else {
            format!("{magnitude:0width$}", width = config.min_integer_digits)
        };
        if let Some(max_d) = config.max_integer_digits
            && int_part.len() > max_d
        {
            int_part = int_part[int_part.len() - max_d..].to_owned();
        }
        let mut full = if value < 0 {
            format!("-{int_part}")
        } else {
            int_part
        };
        if !config.hex && config.min_fraction_digits > 0 {
            full.push('.');
            for _ in 0..config.min_fraction_digits {
                full.push('0');
            }
        }
        full.into_bytes()
    };
    if config.sequential {
        ensure!(config.step > 0, "步长必须为正整数");
        let start = i128::from(config.from);
        let end = i128::from(config.to);
        let step = i128::from(config.step);
        let span = (end - start).unsigned_abs();
        let count_u128 = span / step.unsigned_abs() + 1;
        ensure!(
            count_u128 <= MAX_PAYLOADS as u128,
            "生成数字数量超过 {MAX_PAYLOADS} 上限"
        );
        let count = count_u128 as usize;
        let direction = if config.from <= config.to {
            1i128
        } else {
            -1i128
        };
        let mut out = Vec::with_capacity(count);
        let mut total_bytes = 0usize;
        for idx in 0..count {
            let val = start + (idx as i128) * step * direction;
            let formatted = format_num(val);
            total_bytes = total_bytes
                .checked_add(formatted.len())
                .context("Numbers 总字节数溢出")?;
            ensure_generator_limits(idx + 1, total_bytes, "Numbers")?;
            out.push(formatted);
        }
        Ok(out)
    } else {
        ensure!(
            (1..=MAX_PAYLOADS).contains(&config.how_many),
            "随机生成数字数量必须在 1..={MAX_PAYLOADS} 之间"
        );
        let lo = i128::from(config.from.min(config.to));
        let hi = i128::from(config.from.max(config.to));
        let span = (hi - lo).unsigned_abs().saturating_add(1);
        let mut state = config.seed ^ 0x9e37_79b9_7f4a_7c15;
        let mut out = Vec::with_capacity(config.how_many);
        let mut total_bytes = 0usize;
        for idx in 0..config.how_many {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let offset = if span == 0 {
                0
            } else {
                (u128::from(state) % span) as i128
            };
            let formatted = format_num(lo + offset);
            total_bytes = total_bytes
                .checked_add(formatted.len())
                .context("Numbers 总字节数溢出")?;
            ensure_generator_limits(idx + 1, total_bytes, "Numbers")?;
            out.push(formatted);
        }
        Ok(out)
    }
}

fn format_custom_date_pattern(pat: &str, y: i32, m: u32, d: u32) -> String {
    use std::fmt::Write as _;
    const MONTHS_SHORT: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m_idx = (m.saturating_sub(1) as usize).min(11);
    let yy = y.rem_euclid(100);
    let mut out = String::with_capacity(pat.len() + 4);
    let mut rest = pat;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("yyyy") {
            let _ = write!(out, "{y:04}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("MMM") {
            out.push_str(MONTHS_SHORT[m_idx]);
            rest = after;
        } else if let Some(after) = rest.strip_prefix("MM") {
            let _ = write!(out, "{m:02}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("yy") {
            let _ = write!(out, "{yy:02}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("dd") {
            let _ = write!(out, "{d:02}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("%Y") {
            let _ = write!(out, "{y:04}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("%y") {
            let _ = write!(out, "{yy:02}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("%m") {
            let _ = write!(out, "{m:02}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("%d") {
            let _ = write!(out, "{d:02}");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("%b") {
            out.push_str(MONTHS_SHORT[m_idx]);
            rest = after;
        } else if let Some(ch) = rest.chars().next() {
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

pub fn generate_dates_with_unit_payloads(
    from_date: &str,
    to_date: &str,
    step: u16,
    unit: DateStepUnit,
    format: DateOutputFormat,
    custom_pattern: Option<&str>,
) -> Result<Vec<Vec<u8>>> {
    ensure!(
        (1..=MAX_DATE_STEP_DAYS).contains(&step),
        "日期步长必须在 1..={MAX_DATE_STEP_DAYS} 之间"
    );
    let start_date = parse_iso_date(from_date)?;
    let end_date = parse_iso_date(to_date)?;
    let (fy, fm, fd) = (start_date.year(), start_date.month(), start_date.day());
    let (ty, tm, _) = (end_date.year(), end_date.month(), end_date.day());
    let format_custom = |y: i32, m: u32, d: u32| -> String {
        if let Some(pat) = custom_pattern.filter(|p| !p.trim().is_empty()) {
            format_custom_date_pattern(pat, y, m, d)
        } else {
            format_ymd(y, m, d, format)
        }
    };
    match unit {
        DateStepUnit::Day | DateStepUnit::Week => {
            let mult = if unit == DateStepUnit::Week {
                7u64
            } else {
                1u64
            };
            let step_days = (u64::from(step) * mult) as i64;
            let span = (end_date - start_date).num_days().unsigned_abs();
            let count_u64 = span / (step_days as u64) + 1;
            ensure!(
                count_u64 <= MAX_DATE_SEQUENCE_ITEMS as u64,
                "生成日期数量超过 {MAX_DATE_SEQUENCE_ITEMS} 上限"
            );
            let count = count_u64 as usize;
            let dir = if start_date <= end_date { 1i64 } else { -1i64 };
            let mut out = Vec::with_capacity(count);
            let mut total_bytes = 0usize;
            for idx in 0..count {
                let cur = start_date + TimeDelta::days((idx as i64) * step_days * dir);
                let bytes = format_custom(cur.year(), cur.month(), cur.day()).into_bytes();
                total_bytes = total_bytes
                    .checked_add(bytes.len())
                    .context("Dates payload 总字节数溢出")?;
                ensure_generator_limits(idx + 1, total_bytes, "Dates")?;
                out.push(bytes);
            }
            Ok(out)
        }
        DateStepUnit::Month | DateStepUnit::Year => {
            let start_months = i64::from(fy) * 12 + i64::from(fm) - 1;
            let end_months = i64::from(ty) * 12 + i64::from(tm) - 1;
            let step_months = i64::from(step) * if unit == DateStepUnit::Year { 12 } else { 1 };
            let span = (end_months - start_months).unsigned_abs();
            let count_u64 = span / (step_months as u64) + 1;
            ensure!(
                count_u64 <= MAX_DATE_SEQUENCE_ITEMS as u64,
                "生成日期数量超过 {MAX_DATE_SEQUENCE_ITEMS} 上限"
            );
            let count = count_u64 as usize;
            let dir = if start_months <= end_months {
                1i64
            } else {
                -1i64
            };
            let mut out = Vec::with_capacity(count);
            let mut total_bytes = 0usize;
            for idx in 0..count {
                let cur_m = start_months + (idx as i64) * step_months * dir;
                let y = cur_m.div_euclid(12) as i32;
                let m = (cur_m.rem_euclid(12) + 1) as u32;
                let d = fd.min(days_in_month_chrono(y, m));
                let bytes = format_custom(y, m, d).into_bytes();
                total_bytes = total_bytes
                    .checked_add(bytes.len())
                    .context("Dates payload 总字节数溢出")?;
                ensure_generator_limits(idx + 1, total_bytes, "Dates")?;
                out.push(bytes);
            }
            Ok(out)
        }
    }
}

pub fn generate_brute_forcer_payloads(config: &BruteForcerConfig) -> Result<Vec<Vec<u8>>> {
    let mut chars = Vec::new();
    for ch in config.charset.chars() {
        if !chars.contains(&ch) {
            chars.push(ch);
        }
    }
    ensure!(!chars.is_empty(), "Brute forcer 字符集不能为空");
    ensure!(
        config.min_length <= config.max_length && config.max_length <= 16,
        "Brute forcer 长度范围无效（要求 min <= max <= 16）"
    );
    let radix = chars.len();
    let mut out = Vec::new();
    let mut total_bytes = 0usize;
    for len in config.min_length..=config.max_length {
        if len == 0 {
            total_bytes = total_bytes.checked_add(0).context("字节数溢出")?;
            ensure_generator_limits(out.len() + 1, total_bytes, "Brute forcer")?;
            out.push(Vec::new());
            continue;
        }
        let mut count_for_len = 1usize;
        for _ in 0..len {
            count_for_len = count_for_len
                .checked_mul(radix)
                .context("Brute forcer 组合数量溢出")?;
            ensure!(
                out.len().saturating_add(count_for_len) <= MAX_PAYLOADS,
                "Brute forcer 组合数量超过 {MAX_PAYLOADS} 上限"
            );
        }
        let mut indices = vec![0usize; len];
        for _ in 0..count_for_len {
            let s: String = indices.iter().map(|&idx| chars[idx]).collect();
            let bytes = s.into_bytes();
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .context("Brute forcer 总字节数溢出")?;
            ensure_generator_limits(out.len() + 1, total_bytes, "Brute forcer")?;
            out.push(bytes);
            for pos in (0..len).rev() {
                indices[pos] += 1;
                if indices[pos] < radix {
                    break;
                }
                indices[pos] = 0;
            }
        }
    }
    Ok(out)
}

pub fn generate_character_frobber_payloads(
    config: &CharacterFrobberConfig,
) -> Result<Vec<Vec<u8>>> {
    let base = if config.base_value.is_empty() {
        "admin"
    } else {
        config.base_value.as_str()
    };
    let bytes = base.as_bytes();
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_CHAR_BLOCK_BYTES,
        "Character frobber 基础字符串长度无效"
    );
    let mut out = Vec::with_capacity(bytes.len());
    let mut total_bytes = 0usize;
    for idx in 0..bytes.len() {
        let mut modified = bytes.to_vec();
        modified[idx] = modified[idx].wrapping_add(1);
        total_bytes = total_bytes
            .checked_add(modified.len())
            .context("Character frobber 总字节数溢出")?;
        ensure_generator_limits(out.len() + 1, total_bytes, "Character frobber")?;
        out.push(modified);
    }
    Ok(out)
}

pub fn generate_bit_flipper_payloads(config: &BitFlipperConfig) -> Result<Vec<Vec<u8>>> {
    ensure!(!config.base_value.is_empty(), "Bit flipper 基础值不能为空");
    ensure!(config.bit_mask != 0, "Bit flipper 比特掩码不能为 0");
    let raw_bytes = match config.format {
        BitFlipperFormat::LiteralBytes => config.base_value.as_bytes().to_vec(),
        BitFlipperFormat::AsciiHex => codec_util::hex_decode(&config.base_value)?,
    };
    ensure!(
        !raw_bytes.is_empty() && raw_bytes.len() <= MAX_CHAR_BLOCK_BYTES,
        "Bit flipper 输入字节长度无效"
    );
    let mut out = Vec::new();
    let mut total_bytes = 0usize;
    for byte_idx in 0..raw_bytes.len() {
        for bit in 0..8u8 {
            let mask = 1u8 << bit;
            if (config.bit_mask & mask) == 0 {
                continue;
            }
            let mut flipped = raw_bytes.clone();
            flipped[byte_idx] ^= mask;
            let payload = match config.format {
                BitFlipperFormat::LiteralBytes => flipped,
                BitFlipperFormat::AsciiHex => codec_util::hex_encode(&flipped).into_bytes(),
            };
            total_bytes = total_bytes
                .checked_add(payload.len())
                .context("Bit flipper 总字节数溢出")?;
            ensure_generator_limits(out.len() + 1, total_bytes, "Bit flipper")?;
            out.push(payload);
        }
    }
    Ok(out)
}

pub fn generate_username_payloads(config: &UsernameGeneratorConfig) -> Result<Vec<Vec<u8>>> {
    ensure!(
        !config.names.is_empty(),
        "Username generator 输入姓名列表不能为空"
    );
    let max_per = config.max_per_name.unwrap_or(64).max(1);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    for raw in &config.names {
        let parts: Vec<String> = raw
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .collect();
        if parts.is_empty() {
            continue;
        }
        let mut candidates = Vec::new();
        if parts.len() == 1 {
            let w = &parts[0];
            let cap = capitalize_word(w);
            candidates.extend([w.clone(), w.to_uppercase(), cap]);
        } else {
            let first = &parts[0];
            let last = &parts[parts.len() - 1];
            let f_init: String = first.chars().take(1).collect();
            let l_init: String = last.chars().take(1).collect();
            candidates.extend([
                format!("{first}.{last}"),
                format!("{first}{last}"),
                format!("{first}_{last}"),
                format!("{first}-{last}"),
                format!("{f_init}{last}"),
                format!("{f_init}.{last}"),
                format!("{f_init}_{last}"),
                format!("{f_init}-{last}"),
                format!("{last}{f_init}"),
                format!("{last}.{f_init}"),
                format!("{last}_{f_init}"),
                format!("{last}-{f_init}"),
                format!("{last}.{first}"),
                format!("{last}{first}"),
                format!("{last}_{first}"),
                format!("{last}-{first}"),
                format!("{first}{l_init}"),
                format!("{first}.{l_init}"),
                format!("{first}_{l_init}"),
                first.clone(),
                last.clone(),
                format!("{f_init}{l_init}"),
            ]);
        }
        for candidate in candidates.into_iter().take(max_per) {
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                candidate.into_bytes(),
                "Username generator",
            )?;
        }
    }
    ensure!(!out.is_empty(), "Username generator 未生成任何非空用户名");
    Ok(out)
}

pub fn generate_ecb_block_shuffler_payloads(
    config: &EcbBlockShufflerConfig,
) -> Result<Vec<Vec<u8>>> {
    ensure!(
        matches!(config.block_size, 8 | 16 | 32),
        "ECB 块大小必须为 8、16 或 32 字节"
    );
    let raw_bytes = if config.is_hex {
        codec_util::hex_decode(&config.base_ciphertext)?
    } else {
        config.base_ciphertext.as_bytes().to_vec()
    };
    ensure!(
        !raw_bytes.is_empty() && raw_bytes.len().is_multiple_of(config.block_size),
        "ECB 密文字节长度必须非空且为块大小 ({}) 的整数倍",
        config.block_size
    );
    let blocks: Vec<&[u8]> = raw_bytes.chunks_exact(config.block_size).collect();
    let mut extra_blocks: Vec<Vec<u8>> = Vec::new();
    for extra in &config.additional_encrypted_strings {
        let trimmed = extra.trim();
        if trimmed.is_empty() {
            continue;
        }
        let decoded = if config.is_hex {
            codec_util::hex_decode(trimmed)?
        } else {
            trimmed.as_bytes().to_vec()
        };
        for chunk in decoded.chunks_exact(config.block_size) {
            extra_blocks.push(chunk.to_vec());
        }
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    let serialize_blocks = |b_list: &[&[u8]]| -> Vec<u8> {
        let joined: Vec<u8> = b_list.iter().copied().flatten().copied().collect();
        if config.is_hex {
            codec_util::hex_encode(&joined).into_bytes()
        } else {
            joined
        }
    };
    push_generated_payload(
        &mut out,
        &mut seen,
        &mut total_bytes,
        serialize_blocks(&blocks),
        "ECB block shuffler",
    )?;
    for i in 0..blocks.len() {
        for j in (i + 1)..blocks.len() {
            let mut swapped = blocks.clone();
            swapped.swap(i, j);
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                serialize_blocks(&swapped),
                "ECB block shuffler",
            )?;
        }
    }
    for i in 0..blocks.len() {
        for insert_at in 0..=blocks.len() {
            let mut dup = blocks.clone();
            dup.insert(insert_at, blocks[i]);
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                serialize_blocks(&dup),
                "ECB block shuffler",
            )?;
        }
    }
    if blocks.len() > 1 {
        for i in 0..blocks.len() {
            let mut omitted = blocks.clone();
            omitted.remove(i);
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                serialize_blocks(&omitted),
                "ECB block shuffler",
            )?;
        }
    }
    for extra in &extra_blocks {
        for i in 0..blocks.len() {
            let mut replaced = blocks.clone();
            replaced[i] = extra.as_slice();
            push_generated_payload(
                &mut out,
                &mut seen,
                &mut total_bytes,
                serialize_blocks(&replaced),
                "ECB block shuffler",
            )?;
        }
    }
    Ok(out)
}
