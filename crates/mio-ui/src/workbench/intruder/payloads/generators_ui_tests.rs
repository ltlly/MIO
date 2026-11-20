use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};

use super::super::tests::{attack, intruder_fixture};
use super::generators_ui::{
    generate_payloads_by_type_and_input, parse_case_variants_quick_config,
    parse_char_blocks_quick_config, parse_date_quick_config, parse_empty_repeat_quick_config,
    resolve_processing_rule_with_input,
};
use super::list_ops::encode_selected_chars_in_set;
use crate::workbench::tests::drain;
use mio_core::Command;
use mio_core::intruder::{
    DateOutputFormat, IntruderPayloadType, MAX_PAYLOAD_BYTES, PayloadProcessingRule,
};

#[test]
fn quick_generator_config_parsers_handle_defaults_ranges_and_errors() {
    assert_eq!(parse_empty_repeat_quick_config("").unwrap().count, 10);
    assert_eq!(parse_empty_repeat_quick_config(" 5 ").unwrap().count, 5);
    assert!(parse_empty_repeat_quick_config("bad").is_err());
    let enc_sample = encode_selected_chars_in_set(&[b"a=1&b=2".to_vec()], "=&").unwrap();
    assert_eq!(enc_sample, vec![b"a%3D1%26b%3D2".to_vec()]);
    assert!(encode_selected_chars_in_set(&[b"a".to_vec()], "").is_err());
    assert!(encode_selected_chars_in_set(&[vec![b'&'; MAX_PAYLOAD_BYTES / 2]], "&").is_err());
    let def_date = parse_date_quick_config("", DateOutputFormat::IsoDate).unwrap();
    assert_eq!(
        (
            def_date.from_date.as_str(),
            def_date.to_date.as_str(),
            def_date.step_days
        ),
        ("2026-01-01", "2026-01-07", 1)
    );
    let custom_date =
        parse_date_quick_config("2024-02-28..2024-03-03:2", DateOutputFormat::Compact).unwrap();
    assert_eq!(
        (
            custom_date.from_date.as_str(),
            custom_date.to_date.as_str(),
            custom_date.step_days
        ),
        ("2024-02-28", "2024-03-03", 2)
    );
    assert!(parse_date_quick_config("2024-01-01", DateOutputFormat::IsoDate).is_err());

    let preset_block = parse_char_blocks_quick_config("xy", Some((2, 6, 2))).unwrap();
    assert_eq!(preset_block.base, "xy");
    assert_eq!(preset_block.min_length, 2);
    assert_eq!(preset_block.max_length, 6);

    let spec_block = parse_char_blocks_quick_config("ab:1..5:2", None).unwrap();
    assert_eq!(spec_block.base, "ab");
    assert_eq!(spec_block.min_length, 1);
    assert_eq!(spec_block.max_length, 5);
    assert_eq!(spec_block.step, 2);
    assert!(parse_char_blocks_quick_config("invalid", None).is_err());

    let case_cfg = parse_case_variants_quick_config("userId, apiKey").unwrap();
    assert_eq!(case_cfg.identifiers, vec!["userId", "apiKey"]);
    assert!(parse_case_variants_quick_config(" , ").is_err());

    let (default_sub, clear_sub) = resolve_processing_rule_with_input(
        PayloadProcessingRule::Substring {
            offset: 0,
            length: 8,
        },
        "",
    )
    .unwrap();
    assert!(!clear_sub);
    assert_eq!(
        default_sub,
        PayloadProcessingRule::Substring {
            offset: 0,
            length: 8,
        }
    );
    let (custom_sub, clear_custom) = resolve_processing_rule_with_input(
        PayloadProcessingRule::Substring {
            offset: 0,
            length: 8,
        },
        "1:3",
    )
    .unwrap();
    assert!(clear_custom);
    assert_eq!(
        custom_sub,
        PayloadProcessingRule::Substring {
            offset: 1,
            length: 3,
        }
    );
    assert!(
        resolve_processing_rule_with_input(
            PayloadProcessingRule::MatchReplace {
                match_pattern: String::new(),
                replace_with: String::new(),
            },
            "missing-arrow",
        )
        .is_err()
    );
    let (re_rule, clear_re) = resolve_processing_rule_with_input(
        PayloadProcessingRule::MatchReplaceRegex {
            pattern: String::new(),
            replace_with: String::new(),
        },
        "[0-9]+=>X",
    )
    .unwrap();
    assert!(clear_re);
    assert_eq!(
        re_rule,
        PayloadProcessingRule::MatchReplaceRegex {
            pattern: "[0-9]+".into(),
            replace_with: "X".into(),
        }
    );
    for pt in [
        IntruderPayloadType::CustomIterator,
        IntruderPayloadType::CharacterSubstitution,
        IntruderPayloadType::CaseModification,
        IntruderPayloadType::RecursiveGrep,
        IntruderPayloadType::IllegalUnicode,
        IntruderPayloadType::CharacterBlocks,
        IntruderPayloadType::Numbers,
        IntruderPayloadType::Dates,
        IntruderPayloadType::BruteForcer,
        IntruderPayloadType::NullPayloads,
        IntruderPayloadType::CharacterFrobber,
        IntruderPayloadType::BitFlipper,
        IntruderPayloadType::UsernameGenerator,
        IntruderPayloadType::EcbBlockShuffler,
    ] {
        let generated = generate_payloads_by_type_and_input(pt, "", &[], None).unwrap();
        assert!(!generated.is_empty(), "{pt:?} 应生成非空载荷序列");
    }
}

#[gpui_kit::test]
fn intruder_payload_generator_cards_and_extended_processing_buttons_work(cx: &mut TestAppContext) {
    let mut tab = attack();
    tab.spec.payload_sets = vec![Vec::new()];
    let id = tab.id;
    let mut fixture = intruder_fixture(cx, vec![tab], 64);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .payload_input
            .clone();
        window.scroll(
            "intruder-payloads-panel",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-480.))),
            cx,
        );
        window.render_frame(cx);
        window.click(("input", input.entity_id()), cx);
        window.input("2024-02-28..2024-03-01", cx);
        window.click("intruder-dates-compact", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![
                b"20240228".to_vec(),
                b"20240229".to_vec(),
                b"20240301".to_vec()
            ]
        );
        assert!(input.read(cx).value().is_empty());

        window.click("intruder-payload-clear", cx);
        window.click(("input", input.entity_id()), cx);
        window.input("Ab:2..4:2", cx);
        window.click("intruder-blocks-spec", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"Ab".to_vec(), b"AbAb".to_vec()]
        );

        window.click("intruder-proc-uppercase", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"AB".to_vec(), b"ABAB".to_vec()]
        );

        window.click("intruder-proc-lowercase", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"ab".to_vec(), b"abab".to_vec()]
        );

        window.click("intruder-proc-reverse", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"ba".to_vec(), b"baba".to_vec()]
        );

        window.click("intruder-payload-clear", cx);
        window.click(("input", input.entity_id()), cx);
        window.input("abc", cx);
        window.click("intruder-payload-add", cx);
        window.click("intruder-proc-md5", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"900150983cd24fb0d6963f7d28e17f72".to_vec()]
        );

        window.click("intruder-payload-clear", cx);
        window.click(("input", input.entity_id()), cx);
        window.input("abc", cx);
        window.click("intruder-payload-add", cx);
        window.click("intruder-proc-sha256", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_vec()]
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|cmd| matches!(cmd, Command::SaveWorkspace { .. }))
    );
}

#[gpui_kit::test]
fn intruder_payload_type_dropdown_and_new_processing_rules_and_pipeline_work(
    cx: &mut TestAppContext,
) {
    let mut tab = attack();
    tab.spec.payload_sets = vec![vec![b"<a>12</a>".to_vec()]];
    let id = tab.id;
    let fixture = intruder_fixture(cx, vec![tab], 64);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        assert_eq!(
            window.find("intruder-payload-type-dropdown").label(),
            Some("Simple list")
        );
        fixture.view.update(cx, |view, cx| {
            view.select_intruder_payload_type(
                id,
                IntruderPayloadType::UsernameGenerator,
                window,
                cx,
            );
        });
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-payload-type-dropdown").label(),
            Some("Username generator")
        );
        window.click("intruder-payload-type-generate", cx);
        assert!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0]
                .contains(&b"john.smith".to_vec())
        );

        window.click("intruder-payload-clear", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .payload_input
            .clone();
        input.update(cx, |state, cx| state.set_value("<x>12</x>", window, cx));
        window.click("intruder-payload-add", cx);

        input.update(cx, |state, cx| state.set_value("[0-9]+=>99", window, cx));
        window.click("intruder-proc-regex-replace", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"<x>99</x>".to_vec()]
        );

        window.click("intruder-proc-html-encode", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"&lt;x&gt;99&lt;/x&gt;".to_vec()]
        );
        window.click("intruder-proc-html-decode", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"<x>99</x>".to_vec()]
        );

        window.click("intruder-proc-base64-url-encode", cx);
        window.click("intruder-proc-base64-url-decode", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"<x>99</x>".to_vec()]
        );

        fixture.view.update(cx, |view, cx| {
            view.add_intruder_processing_pipeline_rule(
                id,
                PayloadProcessingRule::Uppercase,
                window,
                cx,
            );
        });
        window.render_frame(cx);
        window.click("intruder-proc-pipeline-run", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"<X>99</X>".to_vec()]
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn intruder_case_variants_and_remaining_seven_processing_rule_buttons_work(
    cx: &mut TestAppContext,
) {
    let mut tab = attack();
    tab.spec.payload_sets = vec![Vec::new()];
    let id = tab.id;
    let mut fixture = intruder_fixture(cx, vec![tab], 64);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .payload_input
            .clone();
        window.scroll(
            "intruder-payloads-panel",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-560.))),
            cx,
        );
        window.render_frame(cx);

        window.click(("input", input.entity_id()), cx);
        window.input("userId", cx);
        window.click("intruder-case-variants", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![
                b"userid".to_vec(),
                b"USERID".to_vec(),
                b"UserId".to_vec(),
                b"userId".to_vec(),
                b"user_id".to_vec(),
                b"USER_ID".to_vec(),
                b"user-id".to_vec(),
            ]
        );
        assert!(input.read(cx).value().is_empty());

        window.click(("input", input.entity_id()), cx);
        window.input("_|-", cx);
        window.click("intruder-proc-skip-if-match", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![
                b"userid".to_vec(),
                b"USERID".to_vec(),
                b"UserId".to_vec(),
                b"userId".to_vec(),
            ]
        );

        window.click(("input", input.entity_id()), cx);
        window.input("0:4", cx);
        window.click("intruder-proc-substring", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![
                b"user".to_vec(),
                b"USER".to_vec(),
                b"User".to_vec(),
                b"user".to_vec(),
            ]
        );

        window.click("intruder-payload-deduplicate", cx);
        window.click(("input", input.entity_id()), cx);
        window.input("user=>root", cx);
        window.click("intruder-proc-replace", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"root".to_vec(), b"USER".to_vec(), b"User".to_vec()]
        );

        window.click(("input", input.entity_id()), cx);
        window.input("^root$=>_v1", cx);
        window.click("intruder-proc-add-if-not-match", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"root".to_vec(), b"USER_v1".to_vec(), b"User_v1".to_vec()]
        );

        window.click("intruder-payload-clear", cx);
        window.click(("input", input.entity_id()), cx);
        window.input("a b/c", cx);
        window.click("intruder-payload-add", cx);
        for (enc, dec, encoded) in [
            (
                "intruder-proc-url-encode",
                "intruder-proc-url-decode",
                &b"a%20b%2Fc"[..],
            ),
            (
                "intruder-proc-base64-encode",
                "intruder-proc-base64-decode",
                &b"YSBiL2M="[..],
            ),
            (
                "intruder-proc-hex-encode",
                "intruder-proc-hex-decode",
                &b"6120622f63"[..],
            ),
        ] {
            window.click(enc, cx);
            assert_eq!(
                fixture.view.read(cx).workspace.intruder.tabs[0]
                    .spec
                    .payload_sets[0],
                vec![encoded.to_vec()]
            );
            window.click(dec, cx);
            assert_eq!(
                fixture.view.read(cx).workspace.intruder.tabs[0]
                    .spec
                    .payload_sets[0],
                vec![b"a b/c".to_vec()]
            );
        }
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|cmd| matches!(cmd, Command::SaveWorkspace { .. }))
    );
}
