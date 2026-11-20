use super::tokens::{detect_encoding, expand_bits, expand_sample_bit_rows, validate_samples};
use super::*;

const GAMMA_EPSILON: f64 = 1e-15;
const GAMMA_CONTINUED_FRACTION_MIN: f64 = 1e-300;
const LN_SQRT_2_PI: f64 = 0.918_938_533_204_672_7;
const LANCZOS_G: f64 = 7.0;
const LANCZOS_COEFFICIENTS: [f64; 9] = [
    0.999_999_999_999_809_9,
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_12,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_6e-7,
];

pub fn analyze_tokens(tokens: &[String]) -> Result<SequencerAnalysis> {
    validate_samples(tokens)?;
    let encoding = detect_encoding(tokens);
    let bits = expand_bits(tokens, encoding)?;
    let position_bit_matrix = expand_sample_bit_rows(tokens, encoding)?;
    let bit_analysis = bit_level_analysis(&bits, &position_bit_matrix);
    let character_analysis = analyze_character_positions(tokens);
    let effective_entropy_bits = character_analysis
        .iter()
        .map(|position| position.effective_entropy_bits)
        .sum();
    let min_length = tokens.iter().map(String::len).min().unwrap_or_default();
    let max_length = tokens.iter().map(String::len).max().unwrap_or_default();
    let charset_size = tokens
        .iter()
        .flat_map(|token| token.chars())
        .collect::<HashSet<_>>()
        .len();
    let quality = quality_rating(tokens.len(), effective_entropy_bits, &bit_analysis.fips);

    Ok(SequencerAnalysis {
        sample_count: tokens.len(),
        min_length,
        max_length,
        charset_size,
        encoding,
        effective_entropy_bits,
        quality,
        diagnosis: build_diagnosis(
            tokens.len(),
            encoding,
            effective_entropy_bits,
            quality,
            &bit_analysis,
        ),
        character_analysis,
        bit_analysis,
    })
}

fn analyze_character_positions(tokens: &[String]) -> Vec<CharacterPositionAnalysis> {
    let max_characters = tokens
        .iter()
        .map(|token| token.chars().count())
        .max()
        .unwrap_or_default();
    let mut frequencies = vec![BTreeMap::<char, usize>::new(); max_characters];

    for token in tokens {
        for (position, character) in token.chars().enumerate() {
            *frequencies[position].entry(character).or_insert(0) += 1;
        }
    }

    frequencies
        .into_iter()
        .enumerate()
        .map(|(position, frequency)| {
            let samples_with_position = frequency.values().sum();
            let observed_characters = frequency.len();
            let shannon_entropy_bits = shannon_entropy(&frequency, samples_with_position);
            let (chi_square, p_value) = uniformity_test(&frequency, samples_with_position);
            let effective_entropy_bits =
                if observed_characters <= 1 || p_value < CHI_SQUARE_SIGNIFICANCE {
                    0.0
                } else {
                    shannon_entropy_bits
                };
            CharacterPositionAnalysis {
                position,
                samples_with_position,
                frequency,
                observed_characters,
                shannon_entropy_bits,
                chi_square,
                degrees_of_freedom: observed_characters.saturating_sub(1),
                p_value,
                effective_entropy_bits,
            }
        })
        .collect()
}

fn shannon_entropy(frequency: &BTreeMap<char, usize>, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    frequency
        .values()
        .filter(|&&count| count > 0)
        .map(|&count| {
            let probability = count as f64 / total as f64;
            -probability * probability.log2()
        })
        .sum()
}

fn uniformity_test(frequency: &BTreeMap<char, usize>, total: usize) -> (f64, f64) {
    let observed_characters = frequency.len();
    if total == 0 || observed_characters <= 1 {
        return (0.0, 1.0);
    }

    let expected = total as f64 / observed_characters as f64;
    let chi_square = frequency
        .values()
        .map(|&count| {
            let difference = count as f64 - expected;
            difference * difference / expected
        })
        .sum();
    let p_value = igammac((observed_characters - 1) as f64 / 2.0, chi_square / 2.0).unwrap_or(0.0);
    (chi_square, p_value)
}

pub fn igammac(a: f64, x: f64) -> Result<f64> {
    ensure!(a.is_finite() && a > 0.0, "igammac requires a > 0");
    ensure!(x.is_finite() && x >= 0.0, "igammac requires x >= 0");
    if x == 0.0 {
        return Ok(1.0);
    }

    let value = if x < a + 1.0 {
        1.0 - lower_regularized_gamma_series(a, x)
    } else {
        upper_regularized_gamma_continued_fraction(a, x)
    };
    Ok(value.clamp(0.0, 1.0))
}

fn lower_regularized_gamma_series(a: f64, x: f64) -> f64 {
    let mut term = 1.0 / a;
    let mut sum = term;
    let mut parameter = a;
    for _ in 0..1_000_000 {
        parameter += 1.0;
        term *= x / parameter;
        sum += term;
        if term.abs() <= sum.abs() * GAMMA_EPSILON {
            break;
        }
    }
    sum * gamma_scaling_factor(a, x)
}

fn upper_regularized_gamma_continued_fraction(a: f64, x: f64) -> f64 {
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / GAMMA_CONTINUED_FRACTION_MIN;
    let mut d = 1.0 / b;
    let mut h = d;
    for iteration in 1..1_000_000 {
        let an = -f64::from(iteration) * (f64::from(iteration) - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < GAMMA_CONTINUED_FRACTION_MIN {
            d = GAMMA_CONTINUED_FRACTION_MIN;
        }
        c = b + an / c;
        if c.abs() < GAMMA_CONTINUED_FRACTION_MIN {
            c = GAMMA_CONTINUED_FRACTION_MIN;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() <= GAMMA_EPSILON {
            break;
        }
    }
    gamma_scaling_factor(a, x) * h
}

fn gamma_scaling_factor(a: f64, x: f64) -> f64 {
    (-x + a * x.ln() - ln_gamma(a)).exp()
}

fn ln_gamma(value: f64) -> f64 {
    if value < 0.5 {
        let reflection = std::f64::consts::PI / (std::f64::consts::PI * value).sin();
        reflection.ln() - ln_gamma(1.0 - value)
    } else {
        let shifted = value - 1.0;
        let series = LANCZOS_COEFFICIENTS[0]
            + LANCZOS_COEFFICIENTS[1..]
                .iter()
                .enumerate()
                .map(|(index, coefficient)| coefficient / (shifted + index as f64 + 1.0))
                .sum::<f64>();
        let t = shifted + LANCZOS_G + 0.5;
        LN_SQRT_2_PI + (shifted + 0.5) * t.ln() - t + series.ln()
    }
}

fn bit_level_analysis(bits: &[bool], bit_rows: &[Vec<bool>]) -> BitAnalysis {
    let analyzed_bits = bits.len().min(MIN_FIPS_TEST_BITS);
    let analyzed = &bits[..analyzed_bits];
    let ones_count = analyzed.iter().filter(|&&bit| bit).count();
    let ones_ratio = if analyzed.is_empty() {
        0.0
    } else {
        ones_count as f64 / analyzed.len() as f64
    };
    let shannon_entropy_bits = binary_shannon_entropy(ones_count, analyzed.len());
    let adjacent_autocorrelation = adjacent_pearson_correlation(bits);
    let (position_ones_ratio, position_shannon_entropy_bits) = per_bit_position_metrics(bit_rows);
    let fips = fips_status(bits);

    BitAnalysis {
        available_bits: bits.len(),
        analyzed_bits,
        ones_count,
        ones_ratio,
        shannon_entropy_bits,
        adjacent_autocorrelation,
        position_ones_ratio,
        position_shannon_entropy_bits,
        fips,
    }
}

fn per_bit_position_metrics(bit_rows: &[Vec<bool>]) -> (Vec<f64>, Vec<f64>) {
    let max_positions = bit_rows.iter().map(Vec::len).max().unwrap_or_default();
    let mut ratios = Vec::with_capacity(max_positions);
    let mut entropies = Vec::with_capacity(max_positions);

    for position in 0..max_positions {
        let mut total = 0_usize;
        let mut ones = 0_usize;
        for row in bit_rows {
            if let Some(&bit) = row.get(position) {
                total += 1;
                if bit {
                    ones += 1;
                }
            }
        }
        let ratio = if total == 0 {
            0.0
        } else {
            ones as f64 / total as f64
        };
        ratios.push(ratio);
        entropies.push(binary_shannon_entropy(ones, total));
    }

    (ratios, entropies)
}

fn binary_shannon_entropy(ones_count: usize, total: usize) -> f64 {
    if total == 0 || ones_count == 0 || ones_count == total {
        return 0.0;
    }
    let ones_probability = ones_count as f64 / total as f64;
    let zeros_probability = 1.0 - ones_probability;
    -ones_probability * ones_probability.log2() - zeros_probability * zeros_probability.log2()
}

fn adjacent_pearson_correlation(bits: &[bool]) -> f64 {
    if bits.len() < 2 {
        return 0.0;
    }
    let pair_count = bits.len() - 1;
    let leading_ones = bits[..pair_count].iter().filter(|&&bit| bit).count();
    let trailing_ones = bits[1..].iter().filter(|&&bit| bit).count();
    let both_ones = bits.windows(2).filter(|pair| pair[0] && pair[1]).count();
    let leading_mean = leading_ones as f64 / pair_count as f64;
    let trailing_mean = trailing_ones as f64 / pair_count as f64;
    let product_mean = both_ones as f64 / pair_count as f64;
    let covariance = product_mean - leading_mean * trailing_mean;
    let leading_variance = leading_mean - leading_mean * leading_mean;
    let trailing_variance = trailing_mean - trailing_mean * trailing_mean;
    let denominator = (leading_variance * trailing_variance).sqrt();
    if denominator == 0.0 {
        0.0
    } else {
        (covariance / denominator).clamp(-1.0, 1.0)
    }
}

fn fips_status(bits: &[bool]) -> FipsStatus {
    if bits.len() < MIN_FIPS_TEST_BITS {
        return FipsStatus::InsufficientBits {
            available_bits: bits.len(),
        };
    }

    let fips_bits = &bits[..MIN_FIPS_TEST_BITS];
    let mut failures = Vec::new();

    let ones_count = fips_bits.iter().filter(|&&bit| bit).count();
    if !(9_726..=10_274).contains(&ones_count) {
        failures.push(FipsFailure {
            test: FipsTest::Monobit,
            observed: format!("{ones_count} ones in {MIN_FIPS_TEST_BITS} bits"),
            requirement: "9726..=10274 ones".to_owned(),
        });
    }

    let mut group_frequency = [0_usize; 16];
    for group in fips_bits.as_chunks::<4>().0 {
        let index = group.iter().fold(0_usize, |accumulator, &bit| {
            (accumulator << 1) | usize::from(bit)
        });
        group_frequency[index] += 1;
    }
    let poker_statistic = 16.0 / 5_000.0
        * group_frequency
            .iter()
            .map(|&count| (count * count) as f64)
            .sum::<f64>()
        - 5_000.0;
    if !(2.16 < poker_statistic && poker_statistic < 46.17) {
        failures.push(FipsFailure {
            test: FipsTest::Poker,
            observed: format!("X={poker_statistic:.4}"),
            requirement: "2.16 < X < 46.17".to_owned(),
        });
    }

    let run_counts = run_counts(fips_bits);
    let run_ranges = [
        (2_315, 2_685),
        (1_114, 1_386),
        (527, 723),
        (240, 384),
        (103, 209),
        (103, 209),
    ];
    let runs_failed = (0..2).any(|bit_value| {
        (0..6).any(|length_index| {
            let (minimum, maximum) = run_ranges[length_index];
            !(minimum..=maximum).contains(&run_counts[bit_value][length_index])
        })
    });
    if runs_failed {
        failures.push(FipsFailure {
            test: FipsTest::Runs,
            observed: format!("zero runs={:?}, one runs={:?}", run_counts[0], run_counts[1]),
            requirement:
                "runs 1..=5 and >=6 in [2315..=2685, 1114..=1386, 527..=723, 240..=384, 103..=209, 103..=209]"
                    .to_owned(),
        });
    }

    let longest = longest_run(fips_bits);
    if longest >= 26 {
        failures.push(FipsFailure {
            test: FipsTest::LongRun,
            observed: format!("longest run={longest}"),
            requirement: "longest run <= 25".to_owned(),
        });
    }

    if failures.is_empty() {
        FipsStatus::Pass
    } else {
        FipsStatus::Failed { failures }
    }
}

fn run_counts(bits: &[bool]) -> [[usize; 6]; 2] {
    let mut counts = [[0_usize; 6]; 2];
    let mut current_bit = bits[0];
    let mut run_length = 1_usize;

    for &bit in &bits[1..] {
        if bit == current_bit {
            run_length += 1;
        } else {
            counts[usize::from(current_bit)][run_length.min(6) - 1] += 1;
            current_bit = bit;
            run_length = 1;
        }
    }
    counts[usize::from(current_bit)][run_length.min(6) - 1] += 1;
    counts
}

fn longest_run(bits: &[bool]) -> usize {
    let mut longest = 1;
    let mut current = 1;
    for pair in bits.windows(2) {
        if pair[0] == pair[1] {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 1;
        }
    }
    longest
}

fn quality_rating(
    sample_count: usize,
    effective_entropy_bits: f64,
    fips: &FipsStatus,
) -> QualityRating {
    if sample_count < RELIABLE_SAMPLE_COUNT {
        QualityRating::InsufficientSample
    } else if matches!(fips, FipsStatus::Failed { .. }) {
        QualityRating::Weak
    } else if effective_entropy_bits >= 112.0 && matches!(fips, FipsStatus::Pass) {
        QualityRating::Excellent
    } else if effective_entropy_bits >= 64.0 {
        QualityRating::Good
    } else if effective_entropy_bits >= 32.0 {
        QualityRating::Fair
    } else {
        QualityRating::Weak
    }
}

fn build_diagnosis(
    sample_count: usize,
    encoding: SampleEncoding,
    effective_entropy_bits: f64,
    quality: QualityRating,
    bits: &BitAnalysis,
) -> String {
    let encoding_label = match encoding {
        SampleEncoding::Hex => "Hex",
        SampleEncoding::Base64 => "Base64",
        SampleEncoding::Ascii => "ASCII",
    };
    let mut diagnosis = format!(
        "共分析 {sample_count} 个样本（{encoding_label}），1% 显著性水平有效熵 {effective_entropy_bits:.2} bits/token，评级 {:?}。",
        quality
    );
    match &bits.fips {
        FipsStatus::Pass => diagnosis.push_str("前 20,000 bits 通过 FIPS 140-2 四项检验。"),
        FipsStatus::Failed { failures } => diagnosis.push_str(&format!(
            "FIPS 140-2 失败 {} 项（{}）。",
            failures.len(),
            failures
                .iter()
                .map(|failure| format!("{:?}", failure.test))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        FipsStatus::InsufficientBits { available_bits } => diagnosis.push_str(&format!(
            "可用比特数 {available_bits} < {MIN_FIPS_TEST_BITS}，未执行 FIPS 140-2 区间判定。"
        )),
    }
    diagnosis
}
