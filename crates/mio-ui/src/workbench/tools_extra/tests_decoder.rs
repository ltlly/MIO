use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, tab};

const GZIPPED_JSON: &[u8] = &[
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xab, 0x56, 0xca, 0xcf, 0x56, 0xb2,
    0x2a, 0x29, 0x2a, 0x4d, 0xd5, 0x51, 0xca, 0x49, 0xcc, 0x4b, 0x57, 0xb2, 0x52, 0x7a, 0xb2, 0x63,
    0xed, 0xb3, 0x69, 0xed, 0x4a, 0xb5, 0x00, 0xa6, 0xa8, 0x4c, 0x52, 0x1b, 0x00, 0x00, 0x00,
];

const BINARY_SAMPLE: &[u8] = b"\x00\xffbinary";

fn base64_encode_std(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn click_btn(f: &Fixture, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
}

fn click_menu_op(
    f: &Fixture,
    menu_id: &'static str,
    item_id: &'static str,
    cx: &mut TestAppContext,
) {
    click_btn(f, menu_id, cx);
    click_btn(f, item_id, cx);
}

fn set_decoder_input(f: &Fixture, value: impl Into<String>, cx: &mut TestAppContext) {
    let value = value.into();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.extra_tools
                .decoder_input
                .update(cx, |state, cx| state.set_value(value, window, cx));
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn decoder_ui_supports_smart_decode_gzip_hashes_and_preserves_raw_bytes(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("Decoder增强")], 64);
    drain(&mut f.commands);
    let tab_id = cx.read(|cx| f.view.read(cx).workspace.tabs[0].id);

    click_btn(&f, "tool-decoder", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("decoder-input-stage").visible());
        assert!(window.find("decoder-input-text-view").visible());
        assert!(window.find("decoder-input-hex-view").visible());
        assert!(window.find("decoder-help-button").visible());
        assert!(window.find("decoder-decode-menu").visible());
        assert!(window.find("decoder-encode-menu").visible());
        assert!(window.find("decoder-hash-menu").visible());
        assert!(window.find("decoder-smart-decode").visible());
        assert!(window.try_find("decoder-output-stage").is_none());
        assert!(window.try_find("decoder-steps-bar").is_none());
        assert!(window.try_find("inspector-sidebar").is_none());
    })
    .unwrap();

    let url_b64_gzip = format!(
        "{}%3D",
        base64_encode_std(GZIPPED_JSON)
            .trim_end_matches('=')
            .replace('/', "%2F")
    );
    set_decoder_input(&f, url_b64_gzip, cx);
    click_btn(&f, "decoder-smart-decode", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("decoder-output-stage").visible());
        assert!(window.find("decoder-steps-bar").visible());
        let view = f.view.read(cx);
        assert_eq!(
            view.extra_tools.decoder_output.read(cx).value().as_ref(),
            r#"{"ok":true,"lang":"中文"}"#
        );
        assert_eq!(
            view.extra_tools.decoder_mode.as_ref(),
            "智能解码 (URL → Base64 → Gzip)"
        );
    })
    .unwrap();

    let b64_gzip = base64_encode_std(GZIPPED_JSON);
    set_decoder_input(&f, b64_gzip, cx);
    click_menu_op(&f, "decoder-decode-menu", "decoder-base64-decode", cx);
    cx.read(|cx| {
        let bytes = f
            .view
            .read(cx)
            .extra_tools
            .decoder_output_bytes
            .clone()
            .expect("binary gzip output must be preserved");
        assert_eq!(bytes, GZIPPED_JSON);
    });
    click_btn(&f, "decoder-continue-button", cx);
    click_menu_op(&f, "decoder-decode-menu", "decoder-gzip-decode", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.extra_tools.decoder_output.read(cx).value().as_ref(),
            r#"{"ok":true,"lang":"中文"}"#
        );
    });

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Repeater;
            view.workspace.active = Some(tab_id);
            if let Some(tab) = view.workspace.tabs.iter_mut().find(|t| t.id == tab_id) {
                tab.draft.raw = BINARY_SAMPLE.to_vec();
                tab.draft.revision += 1;
            }
            if let Some(editors) = view.tab_editors.get_mut(&tab_id) {
                editors.request.load(BINARY_SAMPLE, window, cx);
                editors.loaded_revision = 2;
                editors.dirty = false;
            }
        });
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.send_to_decoder(window, cx);
        });
        window.render_frame(cx);
        let view = f.view.read(cx);
        assert!(view.tool == Tool::Decoder);
        assert_eq!(
            view.extra_tools.decoder_input.read(cx).value().as_ref(),
            "00 ff 62 69 6e 61 72 79"
        );
        assert_eq!(
            view.extra_tools
                .decoder_input_bytes
                .as_ref()
                .map(|(_, bytes)| bytes.clone()),
            Some(BINARY_SAMPLE.to_vec())
        );
    })
    .unwrap();
    cx.run_until_parked();

    click_menu_op(&f, "decoder-hash-menu", "decoder-sha256", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.extra_tools.decoder_output.read(cx).value().as_ref(),
            "821228b5751ba8f20e1b91fb0e2321318969e405fa725da07ad7d6fb272b6091"
        );
        assert_eq!(
            view.extra_tools
                .decoder_input_bytes
                .as_ref()
                .map(|(_, bytes)| bytes.clone()),
            Some(BINARY_SAMPLE.to_vec())
        );
    });

    click_menu_op(&f, "decoder-hash-menu", "decoder-md5", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.extra_tools.decoder_output.read(cx).value().as_ref(),
            "26bb835a2363365c993a230eb260495a"
        );
    });

    click_btn(&f, "decoder-continue-button", cx);
    click_menu_op(&f, "decoder-encode-menu", "decoder-hex-encode", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.extra_tools.decoder_output.read(cx).value().as_ref(),
            "26bb835a2363365c993a230eb260495a"
        );
    });

    click_btn(&f, "decoder-copy-output", cx);
    cx.read(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("26bb835a2363365c993a230eb260495a".into())
        );
    });
}

#[gpui_kit::test]
fn decoder_supports_gzip_compress_octal_binary_sha1_and_step_chain(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("Decoder扩展测试")], 64);
    drain(&mut f.commands);

    click_btn(&f, "tool-decoder", cx);

    set_decoder_input(&f, "abc", cx);
    click_menu_op(&f, "decoder-hash-menu", "decoder-sha1", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .extra_tools
                .decoder_output
                .read(cx)
                .value()
                .as_ref(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    });

    set_decoder_input(&f, "Hi!", cx);
    click_menu_op(&f, "decoder-encode-menu", "decoder-octal-encode", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .extra_tools
                .decoder_output
                .read(cx)
                .value()
                .as_ref(),
            "110 151 041"
        );
    });
    click_btn(&f, "decoder-continue-button", cx);
    click_menu_op(&f, "decoder-decode-menu", "decoder-octal-decode", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .extra_tools
                .decoder_output
                .read(cx)
                .value()
                .as_ref(),
            "Hi!"
        );
    });

    click_btn(&f, "decoder-continue-button", cx);
    click_menu_op(&f, "decoder-encode-menu", "decoder-binary-encode", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .extra_tools
                .decoder_output
                .read(cx)
                .value()
                .as_ref(),
            "01001000 01101001 00100001"
        );
    });
    click_btn(&f, "decoder-continue-button", cx);
    click_menu_op(&f, "decoder-decode-menu", "decoder-binary-decode", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .extra_tools
                .decoder_output
                .read(cx)
                .value()
                .as_ref(),
            "Hi!"
        );
    });

    set_decoder_input(&f, "中文Gzip测试", cx);
    click_menu_op(&f, "decoder-encode-menu", "decoder-gzip-encode", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        let gz_bytes = view
            .extra_tools
            .decoder_output_bytes
            .as_ref()
            .expect("gzip compressed bytes");
        assert!(gz_bytes.starts_with(&[0x1f, 0x8b]));
    });
    click_btn(&f, "decoder-continue-button", cx);
    click_menu_op(&f, "decoder-decode-menu", "decoder-gzip-decode", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .extra_tools
                .decoder_output
                .read(cx)
                .value()
                .as_ref(),
            "中文Gzip测试"
        );
        // Verify cascading step restore and max 16 step bound
        assert_eq!(f.view.read(cx).extra_tools.steps.len(), 7);
    });

    click_btn(&f, ("decoder-step", 1_usize), cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.extra_tools.steps.len(), 2);
        assert_eq!(
            view.extra_tools.decoder_input.read(cx).value().as_ref(),
            "110 151 041"
        );
    });

    for _ in 0..20 {
        click_menu_op(&f, "decoder-encode-menu", "decoder-base64-encode", cx);
    }
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).extra_tools.steps.len(), 16);
    });

    click_btn(&f, "decoder-clear-steps", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let view = f.view.read(cx);
        assert!(view.extra_tools.steps.is_empty());
        assert!(view.extra_tools.decoder_output.read(cx).value().is_empty());
        assert!(window.try_find("decoder-output-stage").is_none());
        assert!(window.try_find("decoder-steps-bar").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn decoder_deflate_digests_hex_views_and_chain_auto_recompute_work(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("Decoder增强链")], 64);
    drain(&mut f.commands);

    click_btn(&f, "tool-decoder", cx);

    set_decoder_input(&f, "hello", cx);
    click_menu_op(&f, "decoder-hash-menu", "decoder-crc32", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_output.read(cx).value().as_ref(),
            "3610a686"
        );
    });
    click_menu_op(&f, "decoder-hash-menu", "decoder-sha384", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_output.read(cx).value().len(),
            96
        );
    });
    click_menu_op(&f, "decoder-hash-menu", "decoder-sha512", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_output.read(cx).value().len(),
            128
        );
    });

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.extra_tools.steps.clear();
            view.extra_tools.decoder_input.update(cx, |state, cx| {
                state.set_value("zlib-payload", window, cx)
            });
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    click_menu_op(&f, "decoder-encode-menu", "decoder-deflate-encode", cx);
    click_btn(&f, "decoder-continue-button", cx);
    click_menu_op(&f, "decoder-decode-menu", "decoder-deflate-decode", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_output.read(cx).value().as_ref(),
            "zlib-payload"
        );
    });

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.extra_tools.steps.clear();
            view.extra_tools.decoder_input_hex_view = false;
            view.extra_tools.decoder_output_hex_view = false;
            view.extra_tools.decoder_input.update(cx, |state, cx| {
                state.set_value("alpha", window, cx)
            });
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    click_menu_op(&f, "decoder-encode-menu", "decoder-base64-url-encode", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_output.read(cx).value().as_ref(),
            "YWxwaGE"
        );
    });

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.extra_tools.decoder_input.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(0..5, cx);
                state.replace("beta", window, cx);
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_output.read(cx).value().as_ref(),
            "YmV0YQ"
        );
    })
    .unwrap();

    click_btn(&f, "decoder-input-hex-view", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_input.read(cx).value().as_ref(),
            "62 65 74 61"
        );
    });
    click_btn(&f, "decoder-output-hex-view", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_output.read(cx).value().as_ref(),
            "59 6d 56 30 59 51"
        );
    });
    click_btn(&f, "decoder-input-text-view", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).extra_tools.decoder_input.read(cx).value().as_ref(),
            "beta"
        );
    });
}
