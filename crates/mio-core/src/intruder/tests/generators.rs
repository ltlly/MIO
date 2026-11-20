use super::*;

#[test]
fn date_sequence_handles_leap_years_formats_reverse_and_limits() {
    let leap = generate_date_sequence_payloads(&DateSequenceConfig {
        from_date: "2024-02-28".into(),
        to_date: "2024-03-01".into(),
        step_days: 1,
        format: DateOutputFormat::IsoDate,
    })
    .unwrap();
    assert_eq!(
        leap,
        vec![
            b"2024-02-28".to_vec(),
            b"2024-02-29".to_vec(),
            b"2024-03-01".to_vec(),
        ]
    );

    let non_leap = generate_date_sequence_payloads(&DateSequenceConfig {
        from_date: "2100-02-28".into(),
        to_date: "2100-03-01".into(),
        step_days: 1,
        format: DateOutputFormat::Compact,
    })
    .unwrap();
    assert_eq!(non_leap, vec![b"21000228".to_vec(), b"21000301".to_vec()]);

    let slash_desc = generate_date_sequence_payloads(&DateSequenceConfig {
        from_date: "2000-03-01".into(),
        to_date: "2000-02-28".into(),
        step_days: 1,
        format: DateOutputFormat::Slash,
    })
    .unwrap();
    assert_eq!(
        slash_desc,
        vec![
            b"2000/03/01".to_vec(),
            b"2000/02/29".to_vec(),
            b"2000/02/28".to_vec(),
        ]
    );

    for bad in [
        DateSequenceConfig {
            from_date: "2023-02-29".into(),
            to_date: "2023-03-01".into(),
            step_days: 1,
            format: DateOutputFormat::IsoDate,
        },
        DateSequenceConfig {
            from_date: "2024-01-01".into(),
            to_date: "2024-01-05".into(),
            step_days: 0,
            format: DateOutputFormat::IsoDate,
        },
        DateSequenceConfig {
            from_date: "2024-01-01".into(),
            to_date: "2024-01-05".into(),
            step_days: 367,
            format: DateOutputFormat::IsoDate,
        },
        DateSequenceConfig {
            from_date: "1990-01-01".into(),
            to_date: "2030-01-01".into(),
            step_days: 1,
            format: DateOutputFormat::IsoDate,
        },
    ] {
        assert!(generate_date_sequence_payloads(&bad).is_err());
    }
}

#[test]
fn character_blocks_and_case_variants_respect_bounds_and_formats() {
    let blocks = generate_character_block_payloads(&CharacterBlocksConfig {
        base: "ab".into(),
        min_length: 1,
        max_length: 5,
        step: 2,
    })
    .unwrap();
    assert_eq!(
        blocks,
        vec![b"a".to_vec(), b"aba".to_vec(), b"ababa".to_vec()]
    );

    let desc_blocks = generate_character_block_payloads(&CharacterBlocksConfig {
        base: "x".into(),
        min_length: 4,
        max_length: 0,
        step: 2,
    })
    .unwrap();
    assert_eq!(
        desc_blocks,
        vec![b"xxxx".to_vec(), b"xx".to_vec(), Vec::<u8>::new()]
    );

    assert!(
        generate_character_block_payloads(&CharacterBlocksConfig {
            base: String::new(),
            min_length: 1,
            max_length: 2,
            step: 1,
        })
        .is_err()
    );
    assert!(
        generate_character_block_payloads(&CharacterBlocksConfig {
            base: "a".into(),
            min_length: 1,
            max_length: MAX_CHAR_BLOCK_BYTES + 1,
            step: 1,
        })
        .is_err()
    );
    assert!(
        generate_character_block_payloads(&CharacterBlocksConfig {
            base: "a".into(),
            min_length: 0,
            max_length: MAX_CHAR_BLOCK_ITEMS,
            step: 1,
        })
        .is_err()
    );

    let variants = generate_case_variant_payloads(&CaseVariantsConfig {
        identifiers: vec!["userProfileId".into()],
    })
    .unwrap();
    let strings: Vec<String> = variants
        .into_iter()
        .map(|b| String::from_utf8(b).unwrap())
        .collect();
    for expected in [
        "userprofileid",
        "USERPROFILEID",
        "UserProfileId",
        "userProfileId",
        "user_profile_id",
        "USER_PROFILE_ID",
        "user-profile-id",
    ] {
        assert!(strings.contains(&expected.to_owned()), "missing {expected}");
    }

    let many_idents: Vec<String> = (0..300).map(|i| format!("field_name_{i}")).collect();
    assert!(
        generate_case_variant_payloads(&CaseVariantsConfig {
            identifiers: many_idents,
        })
        .is_err()
    );
}

#[test]
fn empty_repeats_enforce_count_bounds_and_emit_empty_vectors() {
    use crate::intruder::generators::{EmptyRepeatConfig, generate_empty_repeats};

    assert!(generate_empty_repeats(&EmptyRepeatConfig { count: 0 }).is_err());
    assert!(
        generate_empty_repeats(&EmptyRepeatConfig {
            count: MAX_PAYLOADS + 1,
        })
        .is_err()
    );
    let probe = generate_empty_repeats(&EmptyRepeatConfig { count: 4 }).unwrap();
    assert_eq!(probe, vec![Vec::<u8>::new(); 4]);
    assert_eq!(
        generate_empty_repeats(&EmptyRepeatConfig {
            count: MAX_PAYLOADS,
        })
        .unwrap()
        .len(),
        MAX_PAYLOADS
    );
}

#[test]
fn extended_payload_processing_rules_transform_hash_slice_and_regex_filter() {
    let sample = vec![b"Abc_123".to_vec(), "中文Test".as_bytes().to_vec()];
    assert_eq!(
        apply_payload_processing_rule(&sample, &PayloadProcessingRule::Uppercase).unwrap(),
        vec![b"ABC_123".to_vec(), "中文TEST".as_bytes().to_vec()]
    );
    assert_eq!(
        apply_payload_processing_rule(&sample, &PayloadProcessingRule::Lowercase).unwrap(),
        vec![b"abc_123".to_vec(), "中文test".as_bytes().to_vec()]
    );
    assert_eq!(
        apply_payload_processing_rule(
            &sample,
            &PayloadProcessingRule::Substring {
                offset: 1,
                length: 3,
            }
        )
        .unwrap(),
        vec![b"bc_".to_vec(), "文Te".as_bytes().to_vec()]
    );
    assert_eq!(
        apply_payload_processing_rule(&sample, &PayloadProcessingRule::Reverse).unwrap(),
        vec![b"321_cbA".to_vec(), "tseT文中".as_bytes().to_vec()]
    );
    assert_eq!(
        apply_payload_processing_rule(&[b"abc".to_vec()], &PayloadProcessingRule::Sha256Hex)
            .unwrap(),
        vec![b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_vec()]
    );
    assert_eq!(
        apply_payload_processing_rule(&[b"abc".to_vec()], &PayloadProcessingRule::Md5Hex).unwrap(),
        vec![b"900150983cd24fb0d6963f7d28e17f72".to_vec()]
    );

    let chained = apply_payload_processing_rules(
        &[
            b"alpha".to_vec(),
            b"skip_me".to_vec(),
            b"beta.json".to_vec(),
        ],
        &[
            PayloadProcessingRule::SkipIfMatchesRegex {
                pattern: "^skip_".into(),
            },
            PayloadProcessingRule::AddIfNotMatchRegex {
                pattern: r"\.json$".into(),
                addition: ".json".into(),
            },
            PayloadProcessingRule::Uppercase,
        ],
    )
    .unwrap();
    assert_eq!(chained, vec![b"ALPHA.JSON".to_vec(), b"BETA.JSON".to_vec()]);

    assert!(
        apply_payload_processing_rule(
            &[b"a".to_vec()],
            &PayloadProcessingRule::SkipIfMatchesRegex {
                pattern: "[".into(),
            }
        )
        .is_err()
    );
    let huge_regex = format!("a{{{}}}", 100_000);
    assert!(
        apply_payload_processing_rule(
            &[b"a".to_vec()],
            &PayloadProcessingRule::SkipIfMatchesRegex {
                pattern: huge_regex,
            }
        )
        .is_err()
    );
}

#[test]
fn all_eighteen_payload_types_and_generators_produce_expected_sequences() {
    assert_eq!(IntruderPayloadType::ALL.len(), 18);
    for kind in IntruderPayloadType::ALL {
        assert!(!kind.label().is_empty());
        assert!(!kind.description().is_empty());
    }

    let custom_iter = generate_custom_iterator_payloads(&CustomIteratorConfig {
        slots: vec![
            CustomIteratorSlot {
                prefix: "u=".into(),
                items: vec!["admin".into(), "guest".into()],
                separator: "&".into(),
            },
            CustomIteratorSlot {
                prefix: "p=".into(),
                items: vec!["1".into(), "2".into()],
                separator: String::new(),
            },
        ],
    })
    .unwrap();
    assert_eq!(
        custom_iter,
        vec![
            b"u=admin&p=1".to_vec(),
            b"u=admin&p=2".to_vec(),
            b"u=guest&p=1".to_vec(),
            b"u=guest&p=2".to_vec(),
        ]
    );

    let char_sub = generate_character_substitution_payloads(&CharacterSubstitutionConfig {
        base_items: vec!["ae".into()],
        substitutions: vec![("a".into(), "4".into()), ("e".into(), "3".into())],
    })
    .unwrap();
    assert_eq!(
        char_sub,
        vec![
            b"ae".to_vec(),
            b"4e".to_vec(),
            b"a3".to_vec(),
            b"43".to_vec(),
        ]
    );

    let case_mod = generate_case_modification_payloads(&CaseModificationConfig {
        items: vec!["aBcD".into()],
        mode: CaseModificationMode::AllModes,
    })
    .unwrap();
    assert_eq!(
        case_mod,
        vec![
            b"aBcD".to_vec(),
            b"abcd".to_vec(),
            b"ABCD".to_vec(),
            b"Abcd".to_vec(),
            b"aBCD".to_vec(),
        ]
    );

    let rec_grep = generate_recursive_grep_payloads(
        &RecursiveGrepConfig {
            initial_payloads: vec!["seed0".into()],
            start_delimiter: "<next>".into(),
            end_delimiter: "</next>".into(),
            regex_pattern: None,
            stop_if_duplicate: true,
        },
        &[b"resp1 <next>tok1</next> <next>tok2</next> <next>seed0</next>"],
    )
    .unwrap();
    assert_eq!(
        rec_grep,
        vec![b"seed0".to_vec(), b"tok1".to_vec(), b"tok2".to_vec()]
    );

    let illegal_utf8 = generate_illegal_unicode_payloads(&IllegalUnicodeConfig {
        input: "/".into(),
        overlong_lengths: vec![2, 3],
        hex_encode: false,
        url_encode: true,
        uppercase_hex: true,
    })
    .unwrap();
    assert_eq!(
        illegal_utf8,
        vec![b"%C0%AF".to_vec(), b"%E0%80%AF".to_vec()]
    );

    let nums_seq = generate_numbers_config_payloads(&NumbersGeneratorConfig {
        from: 9,
        to: 11,
        step: 1,
        sequential: true,
        how_many: 3,
        hex: false,
        min_integer_digits: 2,
        max_integer_digits: Some(2),
        min_fraction_digits: 1,
        max_fraction_digits: Some(1),
        seed: 0,
    })
    .unwrap();
    assert_eq!(
        nums_seq,
        vec![b"09.0".to_vec(), b"10.0".to_vec(), b"11.0".to_vec()]
    );

    let dates_month = generate_dates_with_unit_payloads(
        "2024-01-31",
        "2024-03-31",
        1,
        DateStepUnit::Month,
        DateOutputFormat::IsoDate,
        Some("yyyy/MM/dd"),
    )
    .unwrap();
    assert_eq!(
        dates_month,
        vec![
            b"2024/01/31".to_vec(),
            b"2024/02/29".to_vec(),
            b"2024/03/31".to_vec(),
        ]
    );

    let brute = generate_brute_forcer_payloads(&BruteForcerConfig {
        charset: "ab".into(),
        min_length: 1,
        max_length: 2,
    })
    .unwrap();
    assert_eq!(
        brute,
        vec![
            b"a".to_vec(),
            b"b".to_vec(),
            b"aa".to_vec(),
            b"ab".to_vec(),
            b"ba".to_vec(),
            b"bb".to_vec(),
        ]
    );

    let frobber = generate_character_frobber_payloads(&CharacterFrobberConfig {
        base_value: "ab".into(),
    })
    .unwrap();
    assert_eq!(frobber, vec![b"bb".to_vec(), b"ac".to_vec()]);

    let flipper = generate_bit_flipper_payloads(&BitFlipperConfig {
        base_value: "41".into(),
        format: BitFlipperFormat::AsciiHex,
        bit_mask: 0x03,
    })
    .unwrap();
    assert_eq!(flipper, vec![b"40".to_vec(), b"43".to_vec()]);

    let usernames = generate_username_payloads(&UsernameGeneratorConfig {
        names: vec!["John Smith".into()],
        max_per_name: Some(5),
    })
    .unwrap();
    assert_eq!(
        usernames,
        vec![
            b"john.smith".to_vec(),
            b"johnsmith".to_vec(),
            b"john_smith".to_vec(),
            b"john-smith".to_vec(),
            b"jsmith".to_vec(),
        ]
    );

    let ecb = generate_ecb_block_shuffler_payloads(&EcbBlockShufflerConfig {
        base_ciphertext: "000102030405060708090a0b0c0d0e0f".into(),
        is_hex: true,
        block_size: 8,
        additional_encrypted_strings: vec!["ffffffffffffffff".into()],
    })
    .unwrap();
    assert!(ecb.contains(&b"08090a0b0c0d0e0f0001020304050607".to_vec()));
    assert!(ecb.contains(&b"ffffffffffffffff08090a0b0c0d0e0f".to_vec()));
}

#[test]
fn additional_processing_rules_cover_hashes_html_base64url_gzip_and_regex_replace() {
    assert_eq!(
        apply_payload_processing_rule(&[b"abc".to_vec()], &PayloadProcessingRule::Sha1Hex).unwrap(),
        vec![b"a9993e364706816aba3e25717850c26c9cd0d89d".to_vec()]
    );
    assert_eq!(
        apply_payload_processing_rule(&[b"abc".to_vec()], &PayloadProcessingRule::Sha384Hex)
            .unwrap()[0]
            .len(),
        96
    );
    assert_eq!(
        apply_payload_processing_rule(&[b"abc".to_vec()], &PayloadProcessingRule::Sha512Hex)
            .unwrap()[0]
            .len(),
        128
    );

    let html_encoded =
        apply_payload_processing_rule(&[b"<a&\"'>".to_vec()], &PayloadProcessingRule::HtmlEncode)
            .unwrap();
    assert_eq!(html_encoded, vec![b"&lt;a&amp;&quot;&#x27;&gt;".to_vec()]);
    let html_decoded =
        apply_payload_processing_rule(&html_encoded, &PayloadProcessingRule::HtmlDecode).unwrap();
    assert_eq!(html_decoded, vec![b"<a&\"'>".to_vec()]);

    let b64url_encoded = apply_payload_processing_rule(
        &[vec![0xfb, 0xff, 0xfe]],
        &PayloadProcessingRule::Base64UrlEncode,
    )
    .unwrap();
    assert_eq!(b64url_encoded, vec![b"-__-".to_vec()]);
    let b64url_decoded =
        apply_payload_processing_rule(&b64url_encoded, &PayloadProcessingRule::Base64UrlDecode)
            .unwrap();
    assert_eq!(b64url_decoded, vec![vec![0xfb, 0xff, 0xfe]]);

    let gzipped = apply_payload_processing_rule(
        &[b"hello-gzip".to_vec()],
        &PayloadProcessingRule::GzipCompress,
    )
    .unwrap();
    assert_eq!(&gzipped[0][..2], &[0x1f, 0x8b]);

    let regex_replaced = apply_payload_processing_rule(
        &[b"user_123_id_456".to_vec()],
        &PayloadProcessingRule::MatchReplaceRegex {
            pattern: r"\d+".into(),
            replace_with: "X".into(),
        },
    )
    .unwrap();
    assert_eq!(regex_replaced, vec![b"user_X_id_X".to_vec()]);
}
