//! Payload position discovery and lazy binary replacement at byte ranges; no network I/O.
//!
//! Sniper's position-major order and Cluster bomb's last-set-fastest carry define MIO's
//! attack payload placement and permutation schedules.
//! Literal `§` bytes have no special meaning. HTTP normalization belongs to the send layer.

use serde::{Deserialize, Serialize};

mod auto_positions;
pub mod generators;
mod grep;
mod payload_rules;
mod plan;

pub(crate) use auto_positions::{
    AutomaticParameterLimit, MAX_AUTO_BYTES, has_http_parameters, intercept_body_is_structured,
    intercept_multipart_parameters, parse_intercept_content_type,
};
pub use auto_positions::{AutomaticPlacement, automatic_positions};
pub use generators::{
    BitFlipperConfig, BitFlipperFormat, BruteForcerConfig, CaseModificationConfig,
    CaseModificationMode, CaseVariantsConfig, CharacterBlocksConfig, CharacterFrobberConfig,
    CharacterSubstitutionConfig, CopyOtherPayloadConfig, CustomIteratorConfig, CustomIteratorSlot,
    DateOutputFormat, DateSequenceConfig, DateStepUnit, EcbBlockShufflerConfig, EmptyRepeatConfig,
    ExtensionGeneratedConfig, IllegalUnicodeConfig, IntruderPayloadType, MAX_CASE_VARIANTS,
    MAX_CHAR_BLOCK_BYTES, MAX_CHAR_BLOCK_ITEMS, MAX_DATE_SEQUENCE_ITEMS, MAX_DATE_STEP_DAYS,
    NumbersGeneratorConfig, RecursiveGrepConfig, RuntimeFileConfig, UsernameGeneratorConfig,
    generate_bit_flipper_payloads, generate_brute_forcer_payloads,
    generate_case_modification_payloads, generate_case_variant_payloads,
    generate_character_block_payloads, generate_character_frobber_payloads,
    generate_character_substitution_payloads, generate_custom_iterator_payloads,
    generate_date_sequence_payloads, generate_dates_with_unit_payloads,
    generate_ecb_block_shuffler_payloads, generate_empty_repeats,
    generate_illegal_unicode_payloads, generate_numbers_config_payloads,
    generate_recursive_grep_payloads, generate_runtime_file_payloads, generate_username_payloads,
};
pub use grep::{
    evaluate_grep_config, evaluate_grep_extract_pairs, evaluate_grep_payload_hits,
    evaluate_grep_row, find_echoed_inputs,
};
pub use payload_rules::{
    DEFAULT_PAYLOAD_ENCODING_CHARS, apply_payload_processing_rule, apply_payload_processing_rules,
    generate_number_payloads, percent_encode_selected_chars,
};
pub use plan::payload_positions;
pub(crate) use plan::{
    payload_request_count, validate_input_limits, validate_inputs, validate_positions,
};

#[cfg(test)]
use payload_rules::apply_payload_processing_rules_with_limit;
#[cfg(test)]
use plan::{expanded_len, validate_payloads};

/// The documented maximum number of different payload sets, not a position limit.
pub const MAX_PAYLOAD_SETS: usize = 20;

// MIO resource limits, not External defaults. Exceeding one rejects the plan without truncation.
pub const MAX_POSITIONS: usize = 256;
pub const MAX_PAYLOADS: usize = 100_000;
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = crate::http::MAX_BODY + 256 * 1024;
pub const MAX_REQUESTS: u64 = 1_000_000;
pub const MAX_PAYLOAD_RULE_REGEX_BYTES: usize = 256 * 1024;

pub type PayloadProcessRule = PayloadProcessingRule;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttackType {
    Sniper,
    BatteringRam,
    Pitchfork,
    ClusterBomb,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadProcessingRule {
    AddPrefix(String),
    AddSuffix(String),
    Uppercase,
    Lowercase,
    Substring {
        offset: usize,
        length: usize,
    },
    Reverse,
    Sha1Hex,
    Sha256Hex,
    Sha384Hex,
    Sha512Hex,
    Md5Hex,
    MatchReplace {
        match_pattern: String,
        replace_with: String,
    },
    MatchReplaceRegex {
        pattern: String,
        replace_with: String,
    },
    UrlEncode,
    UrlDecode,
    HtmlEncode,
    HtmlDecode,
    Base64Encode,
    Base64Decode,
    Base64UrlEncode,
    Base64UrlDecode,
    AsciiHexEncode,
    AsciiHexDecode,
    GzipCompress,
    AddIfNotMatchRegex {
        pattern: String,
        addition: String,
    },
    SkipIfMatchesRegex {
        pattern: String,
    },
    InvokeExtension {
        processor_name: String,
    },
}

impl PayloadProcessingRule {
    pub fn label(&self) -> String {
        match self {
            Self::AddPrefix(prefix) => format!("Add prefix: {prefix}"),
            Self::AddSuffix(suffix) => format!("Add suffix: {suffix}"),
            Self::Uppercase => "To upper case".into(),
            Self::Lowercase => "To lower case".into(),
            Self::Substring { offset, length } => format!("Substring: {offset}..+{length}"),
            Self::Reverse => "Reverse payload".into(),
            Self::Sha1Hex => "Hash: SHA-1".into(),
            Self::Sha256Hex => "Hash: SHA-256".into(),
            Self::Sha384Hex => "Hash: SHA-384".into(),
            Self::Sha512Hex => "Hash: SHA-512".into(),
            Self::Md5Hex => "Hash: MD5".into(),
            Self::MatchReplace {
                match_pattern,
                replace_with,
            } => format!("Match/replace: {match_pattern} => {replace_with}"),
            Self::MatchReplaceRegex {
                pattern,
                replace_with,
            } => format!("Regex match/replace: {pattern} => {replace_with}"),
            Self::UrlEncode => "Encode: URL-encode".into(),
            Self::UrlDecode => "Decode: URL-decode".into(),
            Self::HtmlEncode => "Encode: HTML-encode".into(),
            Self::HtmlDecode => "Decode: HTML-decode".into(),
            Self::Base64Encode => "Encode: Base64-encode".into(),
            Self::Base64Decode => "Decode: Base64-decode".into(),
            Self::Base64UrlEncode => "Encode: Base64 URL-safe".into(),
            Self::Base64UrlDecode => "Decode: Base64 URL-safe".into(),
            Self::AsciiHexEncode => "Encode: ASCII hex".into(),
            Self::AsciiHexDecode => "Decode: ASCII hex".into(),
            Self::GzipCompress => "Compress: Gzip".into(),
            Self::AddIfNotMatchRegex { pattern, addition } => {
                format!("Add if not match: {pattern} => {addition}")
            }
            Self::SkipIfMatchesRegex { pattern } => format!("Skip if matches: {pattern}"),
            Self::InvokeExtension { processor_name } => {
                format!("Invoke External extension: {processor_name}")
            }
        }
    }
}

/// A half-open range in the original template, not in a preceding generated request.
/// `start == end` inserts bytes. Adjacent/co-located empty ranges keep their supplied order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub start: usize,
    pub end: usize,
}

/// An immutable input selected for one replacement position, before any HTTP normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadPosition {
    pub position: usize,
    pub set_index: usize,
    pub payload_index: usize,
}

/// Owns only the input; neither combinations nor generated requests are retained.
/// Target connection settings stay with the caller and are never inferred from raw bytes.
pub struct AttackPlan {
    kind: AttackType,
    template: Vec<u8>,
    positions: Vec<Position>,
    payload_sets: Vec<Vec<Vec<u8>>>,
    request_count: u64,
}

#[cfg(test)]
mod tests;
