use super::*;

#[test]
fn generate_number_payloads_supports_ascending_descending_decimal_hex_and_padding() {
    assert_eq!(
        generate_number_payloads(-5, 3, 4, 3, false).unwrap(),
        vec![b"-005".to_vec(), b"-001".to_vec(), b"003".to_vec()]
    );
    assert_eq!(
        generate_number_payloads(12, -10, 11, 4, true).unwrap(),
        vec![b"000C".to_vec(), b"0001".to_vec(), b"-000A".to_vec()]
    );
    assert_eq!(
        generate_number_payloads(i64::MIN, i64::MAX, i64::MAX, 1, false).unwrap(),
        vec![
            i64::MIN.to_string().into_bytes(),
            b"-1".to_vec(),
            (i64::MAX - 1).to_string().into_bytes(),
        ]
    );
}

#[test]
fn generate_number_payloads_rejects_invalid_args_and_over_limit_counts() {
    assert!(generate_number_payloads(0, 10, 0, 1, false).is_err());
    assert!(generate_number_payloads(0, 10, -1, 1, false).is_err());
    assert!(generate_number_payloads(0, 10, 1, 0, false).is_err());
    assert!(generate_number_payloads(0, 10, 1, 33, false).is_err());
    assert_eq!(
        generate_number_payloads(1, MAX_PAYLOADS as i64, 1, 1, false)
            .unwrap()
            .len(),
        MAX_PAYLOADS
    );
    assert!(generate_number_payloads(0, MAX_PAYLOADS as i64, 1, 1, false).is_err());
}

#[test]
fn apply_payload_processing_rule_transforms_all_variants_and_validates_limits() {
    let initial = vec![b"a b+c".to_vec(), b"\xff\x00".to_vec()];
    let prefixed =
        apply_payload_processing_rule(&initial, &PayloadProcessingRule::AddPrefix("pre:".into()))
            .unwrap();
    assert_eq!(
        prefixed,
        vec![b"pre:a b+c".to_vec(), b"pre:\xff\x00".to_vec()]
    );

    let suffixed =
        apply_payload_processing_rule(&prefixed, &PayloadProcessingRule::AddSuffix(":suf".into()))
            .unwrap();
    assert_eq!(
        suffixed,
        vec![b"pre:a b+c:suf".to_vec(), b"pre:\xff\x00:suf".to_vec()]
    );

    let replaced = apply_payload_processing_rule(
        &suffixed,
        &PayloadProcessingRule::MatchReplace {
            match_pattern: "a b".into(),
            replace_with: "A_B".into(),
        },
    )
    .unwrap();
    assert_eq!(
        replaced,
        vec![b"pre:A_B+c:suf".to_vec(), b"pre:\xff\x00:suf".to_vec()]
    );
    assert!(
        apply_payload_processing_rule(
            &initial,
            &PayloadProcessingRule::MatchReplace {
                match_pattern: String::new(),
                replace_with: "x".into(),
            },
        )
        .is_err()
    );

    let url_encoded =
        apply_payload_processing_rule(&initial, &PayloadProcessingRule::UrlEncode).unwrap();
    assert_eq!(url_encoded, vec![b"a%20b%2Bc".to_vec(), b"%FF%00".to_vec()]);
    let url_decoded = apply_payload_processing_rule(
        &[b"a+b%2Bc%ff%00".to_vec()],
        &PayloadProcessingRule::UrlDecode,
    )
    .unwrap();
    assert_eq!(url_decoded, vec![b"a b+c\xff\x00".to_vec()]);
    assert!(
        apply_payload_processing_rule(&[b"%2".to_vec()], &PayloadProcessingRule::UrlDecode)
            .is_err()
    );
    assert!(
        apply_payload_processing_rule(&[b"%GG".to_vec()], &PayloadProcessingRule::UrlDecode)
            .is_err()
    );

    let b64_inputs = vec![
        b"".to_vec(),
        b"f".to_vec(),
        b"fo".to_vec(),
        b"foo".to_vec(),
        b"\xff\x00".to_vec(),
    ];
    let b64_encoded =
        apply_payload_processing_rule(&b64_inputs, &PayloadProcessingRule::Base64Encode).unwrap();
    assert_eq!(
        b64_encoded,
        vec![
            b"".to_vec(),
            b"Zg==".to_vec(),
            b"Zm8=".to_vec(),
            b"Zm9v".to_vec(),
            b"/wA=".to_vec(),
        ]
    );
    let b64_decoded =
        apply_payload_processing_rule(&b64_encoded, &PayloadProcessingRule::Base64Decode).unwrap();
    assert_eq!(b64_decoded, b64_inputs);
    for bad_b64 in [&b"abc"[..], &b"===="[..], &b"Zg=a"[..], &b"Z$=="[..]] {
        assert!(
            apply_payload_processing_rule(
                &[bad_b64.to_vec()],
                &PayloadProcessingRule::Base64Decode
            )
            .is_err()
        );
    }

    let hex_encoded =
        apply_payload_processing_rule(&initial, &PayloadProcessingRule::AsciiHexEncode).unwrap();
    assert_eq!(hex_encoded, vec![b"6120622b63".to_vec(), b"ff00".to_vec()]);
    let hex_decoded = apply_payload_processing_rule(
        &[b"61 20\t62\n2B63".to_vec(), b"FF00".to_vec()],
        &PayloadProcessingRule::AsciiHexDecode,
    )
    .unwrap();
    assert_eq!(hex_decoded, initial);
    assert!(
        apply_payload_processing_rule(&[b"abc".to_vec()], &PayloadProcessingRule::AsciiHexDecode)
            .is_err()
    );
    assert!(
        apply_payload_processing_rule(&[b"zz".to_vec()], &PayloadProcessingRule::AsciiHexDecode)
            .is_err()
    );

    let too_many = vec![Vec::new(); MAX_PAYLOADS + 1];
    assert!(apply_payload_processing_rule(&too_many, &PayloadProcessingRule::UrlEncode).is_err());
}

#[test]
fn payload_processing_literal_replace_preserves_binary_and_nonoverlapping_matches() {
    let input = vec![b"\xffaaa\0aa\xfe".to_vec(), "中\0文".as_bytes().to_vec()];
    assert_eq!(
        apply_payload_processing_rule(
            &input,
            &PayloadProcessingRule::MatchReplace {
                match_pattern: "aa".into(),
                replace_with: "\0x".into(),
            },
        )
        .unwrap(),
        vec![b"\xff\0xa\0\0x\xfe".to_vec(), "中\0文".as_bytes().to_vec()]
    );
    assert_eq!(
        apply_payload_processing_rules(
            &input,
            &[
                PayloadProcessingRule::AddPrefix("前\0".into()),
                PayloadProcessingRule::MatchReplace {
                    match_pattern: "\0".into(),
                    replace_with: "_".into(),
                },
                PayloadProcessingRule::AddSuffix("尾".into()),
            ],
        )
        .unwrap(),
        vec![
            ["前_".as_bytes(), b"\xffaaa_aa\xfe", "尾".as_bytes()].concat(),
            "前_中_文尾".as_bytes().to_vec(),
        ]
    );
    assert_eq!(apply_payload_processing_rules(&input, &[]).unwrap(), input);
}

#[test]
fn payload_processing_validates_full_input_and_every_stage_before_expansion() {
    for (input, rule, output) in [
        (
            b"ab".as_slice(),
            PayloadProcessingRule::AddPrefix("\0p".into()),
            b"\0pab".as_slice(),
        ),
        (
            b"ab".as_slice(),
            PayloadProcessingRule::AddSuffix("\0s".into()),
            b"ab\0s".as_slice(),
        ),
        (
            b"\xff-".as_slice(),
            PayloadProcessingRule::UrlEncode,
            b"%FF-".as_slice(),
        ),
        (
            b"fo".as_slice(),
            PayloadProcessingRule::Base64Encode,
            b"Zm8=".as_slice(),
        ),
        (
            b"\xff\0".as_slice(),
            PayloadProcessingRule::AsciiHexEncode,
            b"ff00".as_slice(),
        ),
        (
            b"aa".as_slice(),
            PayloadProcessingRule::MatchReplace {
                match_pattern: "a".into(),
                replace_with: "xy".into(),
            },
            b"xyxy".as_slice(),
        ),
    ] {
        let values = vec![input.to_vec()];
        assert_eq!(
            apply_payload_processing_rules_with_limit(&values, std::slice::from_ref(&rule), 4)
                .unwrap(),
            vec![output.to_vec()]
        );
        assert!(apply_payload_processing_rules_with_limit(&values, &[rule], 3).is_err());
        assert_eq!(values, vec![input.to_vec()]);
    }
    let input = vec![b"a".to_vec(), b"a".to_vec()];
    let expand = PayloadProcessingRule::MatchReplace {
        match_pattern: "a".into(),
        replace_with: "abc".into(),
    };
    assert!(
        apply_payload_processing_rules_with_limit(&input, std::slice::from_ref(&expand), 5)
            .is_err()
    );
    assert_eq!(
        apply_payload_processing_rules_with_limit(&input, std::slice::from_ref(&expand), 6)
            .unwrap(),
        vec![b"abc".to_vec(), b"abc".to_vec()]
    );
    assert!(
        apply_payload_processing_rules_with_limit(
            &input,
            &[
                expand,
                PayloadProcessingRule::MatchReplace {
                    match_pattern: "abc".into(),
                    replace_with: "a".into(),
                },
            ],
            5,
        )
        .is_err()
    );
    for (value, rule) in [
        (b"%00%01".as_slice(), PayloadProcessingRule::UrlDecode),
        (b"Zm9vYg==".as_slice(), PayloadProcessingRule::Base64Decode),
        (
            b"00 01 02".as_slice(),
            PayloadProcessingRule::AsciiHexDecode,
        ),
        (
            b"ababab".as_slice(),
            PayloadProcessingRule::MatchReplace {
                match_pattern: "ab".into(),
                replace_with: String::new(),
            },
        ),
    ] {
        assert!(apply_payload_processing_rules_with_limit(&[value.to_vec()], &[rule], 5).is_err());
    }
    assert!(apply_payload_processing_rules_with_limit(&[b"abcdef".to_vec()], &[], 5).is_err());
}

#[test]
fn payload_processing_invalid_decodes_are_atomic_and_base64_requires_canonical_padding() {
    for (valid, invalid, rule) in [
        (
            b"00 ff".as_slice(),
            b"00 fg".as_slice(),
            PayloadProcessingRule::AsciiHexDecode,
        ),
        (
            b"00 ff".as_slice(),
            b"00 f".as_slice(),
            PayloadProcessingRule::AsciiHexDecode,
        ),
        (
            b"%00".as_slice(),
            b"%ff%Q0".as_slice(),
            PayloadProcessingRule::UrlDecode,
        ),
        (
            b"Zg==".as_slice(),
            b"Zh==".as_slice(),
            PayloadProcessingRule::Base64Decode,
        ),
        (
            b"Zm8=".as_slice(),
            b"Zm9=".as_slice(),
            PayloadProcessingRule::Base64Decode,
        ),
        (
            b"Zm9v".as_slice(),
            b"Zg==Zm9v".as_slice(),
            PayloadProcessingRule::Base64Decode,
        ),
        (
            b"Zg==".as_slice(),
            b"Zg".as_slice(),
            PayloadProcessingRule::Base64Decode,
        ),
        (
            b"Zg==".as_slice(),
            b"Zg= ".as_slice(),
            PayloadProcessingRule::Base64Decode,
        ),
    ] {
        let values = vec![valid.to_vec(), invalid.to_vec()];
        assert!(apply_payload_processing_rule(&values, &rule).is_err());
        assert_eq!(values, vec![valid.to_vec(), invalid.to_vec()]);
        assert!(
            apply_payload_processing_rules(
                &values,
                &[rule, PayloadProcessingRule::AddSuffix("\0".into())],
            )
            .is_err()
        );
        assert_eq!(values, vec![valid.to_vec(), invalid.to_vec()]);
    }
    for (encoded_tail, decoded_tail) in [
        (b"".as_slice(), b"".as_slice()),
        (b"Zg==".as_slice(), b"f".as_slice()),
        (b"Zm8=".as_slice(), b"fo".as_slice()),
    ] {
        let encoded = [b"Zm9v".repeat(17), encoded_tail.to_vec()].concat();
        let decoded = [b"foo".repeat(17), decoded_tail.to_vec()].concat();
        assert_eq!(
            apply_payload_processing_rule(&[encoded], &PayloadProcessingRule::Base64Decode)
                .unwrap(),
            vec![decoded]
        );
    }
}

#[test]
fn percent_encode_selected_chars_encodes_only_requested_bytes_and_preserves_binary() {
    use crate::intruder::payload_rules::{
        DEFAULT_PAYLOAD_ENCODING_CHARS, percent_encode_selected_chars,
    };

    let input = b"a=1&b=hello world/path?q=#[x]\xff";
    assert_eq!(
        percent_encode_selected_chars(input, DEFAULT_PAYLOAD_ENCODING_CHARS),
        b"a%3D1%26b%3Dhello%20world%2Fpath%3Fq%3D%23%5Bx%5D\xff"
    );
    assert_eq!(percent_encode_selected_chars(input, ""), input);
    assert_eq!(
        percent_encode_selected_chars("中文&a=b".as_bytes(), "&="),
        "中文%26a%3Db".as_bytes()
    );
}
