use super::*;
use crate::HttpVersion;

fn response(headers: Vec<(&str, &[u8])>, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: headers
            .into_iter()
            .map(|(name, value)| Header {
                name: name.to_owned(),
                value: value.to_vec(),
            })
            .collect(),
        body: body.to_vec(),
        trailers: Vec::new(),
    }
}

fn xorshift64_star(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

#[test]
fn extracts_cookie_header_and_body_delimiter_tokens() {
    let latin1_header = [b't', b'o', b'k', b'-', 0xe9];
    let resp = response(
        vec![
            (
                "Set-Cookie",
                b"theme=dark; Max-Age=3600; sid=wrong".as_slice(),
            ),
            (
                "set-cookie",
                b"  sid = tok_98765 ; HttpOnly; Path=/".as_slice(),
            ),
            ("X-CSRF-Token", &latin1_header),
        ],
        b"<html><input name=\"csrf\" value=\"tok_body_42\" /><input value=\"later\" /></html>",
    );

    assert_eq!(
        extract_token(
            &resp,
            &SequencerTokenLocation::Cookie {
                name: "sid".to_owned(),
            },
        ),
        Some("tok_98765".to_owned())
    );
    assert_eq!(
        extract_token(
            &resp,
            &SequencerTokenLocation::Cookie {
                name: "Max-Age".to_owned(),
            },
        ),
        None
    );
    assert_eq!(
        extract_token(
            &resp,
            &SequencerTokenLocation::Header {
                name: "x-csrf-token".to_owned(),
            },
        ),
        Some("tok-\u{e9}".to_owned())
    );
    assert_eq!(
        extract_token(
            &resp,
            &SequencerTokenLocation::BodyDelimiter {
                start: "value=\"".to_owned(),
                end: "\"".to_owned(),
            },
        ),
        Some("tok_body_42".to_owned())
    );

    let oversized_body = format!("<t>{}</t>", "a".repeat(MAX_TOKEN_BYTES + 1));
    let empty_or_oversized = response(Vec::new(), oversized_body.as_bytes());
    assert_eq!(
        extract_token(
            &empty_or_oversized,
            &SequencerTokenLocation::BodyDelimiter {
                start: "<t>".to_owned(),
                end: "</t>".to_owned(),
            },
        ),
        None
    );
}

#[test]
fn incrementing_session_tokens_have_very_low_effective_entropy_and_rate_weak() {
    let tokens = (1..=500)
        .map(|index| format!("session_{index:04}"))
        .collect::<Vec<_>>();
    let analysis = analyze_tokens(&tokens).expect("valid sample set");

    assert_eq!(analysis.sample_count, 500);
    assert_eq!(analysis.min_length, 12);
    assert_eq!(analysis.max_length, 12);
    assert_eq!(analysis.encoding, SampleEncoding::Ascii);
    for constant_position in 0..8 {
        assert_eq!(
            analysis.character_analysis[constant_position].effective_entropy_bits,
            0.0
        );
    }
    assert!(
        analysis.effective_entropy_bits < 10.0,
        "expected very low effective entropy, got {}",
        analysis.effective_entropy_bits
    );
    assert_eq!(analysis.quality, QualityRating::Weak);
    assert!(matches!(
        analysis.bit_analysis.fips,
        FipsStatus::InsufficientBits { .. }
    ));
    assert!(!analysis.bit_analysis.position_ones_ratio.is_empty());
}

#[test]
fn high_entropy_pseudorandom_hex_tokens_pass_fips_and_approach_theoretical_entropy() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let tokens = (0..500)
        .map(|_| {
            let high = xorshift64_star(&mut state);
            let low = xorshift64_star(&mut state);
            format!("{high:016x}{low:016x}")
        })
        .collect::<Vec<_>>();
    let analysis = analyze_tokens(&tokens).expect("valid high-entropy samples");

    assert_eq!(analysis.encoding, SampleEncoding::Hex);
    assert_eq!(analysis.charset_size, 16);
    assert_eq!(analysis.bit_analysis.available_bits, 64_000);
    assert_eq!(analysis.bit_analysis.analyzed_bits, MIN_FIPS_TEST_BITS);
    assert_eq!(analysis.bit_analysis.fips, FipsStatus::Pass);
    assert_eq!(analysis.quality, QualityRating::Excellent);
    assert!(
        analysis.effective_entropy_bits > 120.0,
        "expected near-128-bit effective entropy, got {}",
        analysis.effective_entropy_bits
    );
    for position in &analysis.character_analysis {
        assert!(
            (3.85..=4.0).contains(&position.shannon_entropy_bits),
            "position {} entropy {} outside expected 4-bit hex range",
            position.position,
            position.shannon_entropy_bits
        );
        assert!(position.p_value >= CHI_SQUARE_SIGNIFICANCE);
    }
    assert!(analysis.bit_analysis.adjacent_autocorrelation.abs() < 0.05);
    assert!(analysis.bit_analysis.shannon_entropy_bits > 0.999);
}

#[test]
fn all_zero_and_periodic_streams_fail_all_four_fips_tests() {
    let expected = vec![
        FipsTest::Monobit,
        FipsTest::Poker,
        FipsTest::Runs,
        FipsTest::LongRun,
    ];

    let all_zero = vec!["0".repeat(40); 125];
    let zero_analysis = analyze_tokens(&all_zero).expect("valid zero samples");
    let FipsStatus::Failed { failures } = zero_analysis.bit_analysis.fips else {
        panic!("all-zero stream must fail FIPS 140-2");
    };
    assert_eq!(
        failures.iter().map(|item| item.test).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(zero_analysis.quality, QualityRating::Weak);

    let periodic = vec!["00000001".repeat(5); 125];
    let periodic_analysis = analyze_tokens(&periodic).expect("valid periodic samples");
    let FipsStatus::Failed { failures } = periodic_analysis.bit_analysis.fips else {
        panic!("periodic stream must fail FIPS 140-2");
    };
    assert_eq!(
        failures.iter().map(|item| item.test).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(periodic_analysis.quality, QualityRating::Weak);
}

#[test]
fn base64_and_igammac_reference_values_are_accurate() {
    let tokens = vec!["AQIDBA==".to_owned(), "+/8AAA==".to_owned()];
    let analysis = analyze_tokens(&tokens).expect("valid Base64 tokens");
    assert_eq!(analysis.encoding, SampleEncoding::Base64);
    assert_eq!(analysis.quality, QualityRating::InsufficientSample);
    assert_eq!(
        analysis.bit_analysis.fips,
        FipsStatus::InsufficientBits { available_bits: 64 }
    );
    assert_eq!(analysis.bit_analysis.position_ones_ratio.len(), 32);

    let p_df2 = igammac(1.0, 2.3).expect("valid gamma arguments");
    assert!((p_df2 - (-2.3_f64).exp()).abs() < 1e-12);
    let p_large = igammac(7.5, 22.0).expect("valid continued fraction branch");
    assert!((0.0..1e-3).contains(&p_large));
}

#[test]
fn uuidv4_and_unpadded_base64url_tokens_pass_fips_without_framing_bias() {
    const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let encode_b64url = |bytes: &[u8]| -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        let mut i = 0;
        while i + 3 <= bytes.len() {
            let n = (u32::from(bytes[i]) << 16)
                | (u32::from(bytes[i + 1]) << 8)
                | u32::from(bytes[i + 2]);
            out.push(B64URL[((n >> 18) & 63) as usize] as char);
            out.push(B64URL[((n >> 12) & 63) as usize] as char);
            out.push(B64URL[((n >> 6) & 63) as usize] as char);
            out.push(B64URL[(n & 63) as usize] as char);
            i += 3;
        }
        if i + 1 == bytes.len() {
            let n = u32::from(bytes[i]) << 4;
            out.push(B64URL[((n >> 6) & 63) as usize] as char);
            out.push(B64URL[(n & 63) as usize] as char);
        }
        out
    };

    let mut state = 0x243f_6a88_85a3_08d3_u64;
    let uuid_tokens: Vec<String> = (0..250)
        .map(|_| {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&xorshift64_star(&mut state).to_be_bytes());
            b[8..].copy_from_slice(&xorshift64_star(&mut state).to_be_bytes());
            b[6] = (b[6] & 0x0f) | 0x40;
            b[8] = (b[8] & 0x3f) | 0x80;
            crate::Uuid::from_bytes(b).to_string()
        })
        .collect();
    let uuid_analysis = analyze_tokens(&uuid_tokens).unwrap();
    assert_eq!(uuid_analysis.encoding, SampleEncoding::Hex);
    assert_eq!(uuid_analysis.bit_analysis.fips, FipsStatus::Pass);
    assert_eq!(uuid_analysis.quality, QualityRating::Excellent);

    let zero_uuids = vec![crate::Uuid::nil().to_string(); 200];
    let zero_uuid_analysis = analyze_tokens(&zero_uuids).unwrap();
    assert_eq!(zero_uuid_analysis.quality, QualityRating::Weak);

    let b64url_tokens: Vec<String> = (0..250)
        .map(|_| {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&xorshift64_star(&mut state).to_be_bytes());
            b[8..].copy_from_slice(&xorshift64_star(&mut state).to_be_bytes());
            encode_b64url(&b)
        })
        .collect();
    let b64_analysis = analyze_tokens(&b64url_tokens).unwrap();
    assert_eq!(b64_analysis.encoding, SampleEncoding::Base64);
    assert_eq!(b64_analysis.bit_analysis.fips, FipsStatus::Pass);
    assert_eq!(b64_analysis.quality, QualityRating::Excellent);
}

#[test]
fn regex_token_extraction_and_compression_helpers_roundtrip() {
    let response = crate::HttpResponse {
        version: crate::HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![crate::Header {
            name: "X-Auth".into(),
            value: b"Bearer tok_regex_9988".to_vec(),
        }],
        body: br#"{"csrf":"body_tok_7766"}"#.to_vec(),
        trailers: Vec::new(),
    };
    assert_eq!(
        extract_token(
            &response,
            &SequencerTokenLocation::Regex {
                pattern: r"Bearer\s+([A-Za-z0-9_]+)".into(),
            }
        ),
        Some("tok_regex_9988".into())
    );
    assert_eq!(
        extract_token(
            &response,
            &SequencerTokenLocation::Regex {
                pattern: r#""csrf":"([^"]+)""#.into(),
            }
        ),
        Some("body_tok_7766".into())
    );
    let dual_order_pattern = r#"(?is)<input\b(?:[^>]*\bname\s*=\s*["']?csrf["']?[^>]*\bvalue\s*=\s*["']([^"'<>]+)["']|[^>]*\bvalue\s*=\s*["']([^"'<>]+)["'][^>]*\bname\s*=\s*["']?csrf["']?)"#.to_owned();
    let form_name_first = crate::HttpResponse {
        body: br#"<form><input type="hidden" name="csrf" value="tok_name_first"></form>"#.to_vec(),
        ..response.clone()
    };
    let form_value_first = crate::HttpResponse {
        body: br#"<form><input value="tok_value_first" type="hidden" name="csrf"></form>"#.to_vec(),
        ..response
    };
    assert_eq!(
        extract_token(
            &form_name_first,
            &SequencerTokenLocation::Regex {
                pattern: dual_order_pattern.clone(),
            }
        ),
        Some("tok_name_first".into())
    );
    assert_eq!(
        extract_token(
            &form_value_first,
            &SequencerTokenLocation::Regex {
                pattern: dual_order_pattern,
            }
        ),
        Some("tok_value_first".into())
    );

    let payload = b"deflate and gzip roundtrip payload";
    let deflated = crate::compress_deflate_bytes(payload, 4096).unwrap();
    assert_eq!(crate::decompress_deflate_bytes(&deflated, 4096).unwrap(), payload);
    let gzipped = crate::compress_gzip_bytes(payload, 4096).unwrap();
    assert_eq!(crate::decompress_gzip_bytes(&gzipped, 4096).unwrap(), payload);
    let brotli_empty = &[0x06];
    assert!(crate::decompress_brotli_bytes(brotli_empty, 4096).unwrap().is_empty());
}
