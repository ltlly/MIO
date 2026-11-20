//! Sequencer token extraction and deterministic randomness statistics.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{Header, HttpResponse};

mod stats;
mod tokens;

#[cfg(test)]
mod tests;

pub use stats::{analyze_tokens, igammac};
pub use tokens::{expand_sample_bits, extract_token};

pub const MIN_SAMPLE_COUNT: usize = 2;
pub const MAX_SAMPLE_COUNT: usize = 20_000;
pub const MIN_TOKEN_BYTES: usize = 1;
pub const MAX_TOKEN_BYTES: usize = 1024;
pub const MAX_TOTAL_SAMPLE_BYTES: usize = 8 * 1024 * 1024;
pub const MIN_FIPS_TEST_BITS: usize = 20_000;
pub const RELIABLE_SAMPLE_COUNT: usize = 100;

const CHI_SQUARE_SIGNIFICANCE: f64 = 0.01;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SequencerTokenLocation {
    Cookie { name: String },
    Header { name: String },
    BodyDelimiter { start: String, end: String },
    Regex { pattern: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleEncoding {
    Ascii,
    Hex,
    Base64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityRating {
    Excellent,
    Good,
    Fair,
    Weak,
    InsufficientSample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FipsTest {
    Monobit,
    Poker,
    Runs,
    LongRun,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FipsFailure {
    pub test: FipsTest,
    pub observed: String,
    pub requirement: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum FipsStatus {
    Pass,
    Failed { failures: Vec<FipsFailure> },
    InsufficientBits { available_bits: usize },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterPositionAnalysis {
    pub position: usize,
    pub samples_with_position: usize,
    pub frequency: BTreeMap<char, usize>,
    pub observed_characters: usize,
    pub shannon_entropy_bits: f64,
    pub chi_square: f64,
    pub degrees_of_freedom: usize,
    pub p_value: f64,
    pub effective_entropy_bits: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BitAnalysis {
    pub available_bits: usize,
    pub analyzed_bits: usize,
    pub ones_count: usize,
    pub ones_ratio: f64,
    pub shannon_entropy_bits: f64,
    pub adjacent_autocorrelation: f64,
    pub position_ones_ratio: Vec<f64>,
    pub position_shannon_entropy_bits: Vec<f64>,
    pub fips: FipsStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencerAnalysis {
    pub sample_count: usize,
    pub min_length: usize,
    pub max_length: usize,
    pub charset_size: usize,
    pub encoding: SampleEncoding,
    pub effective_entropy_bits: f64,
    pub quality: QualityRating,
    pub diagnosis: String,
    pub character_analysis: Vec<CharacterPositionAnalysis>,
    pub bit_analysis: BitAnalysis,
}
