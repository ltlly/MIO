use super::*;
use crate::workbench::tests::{drain, fixture, tab};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use mio_core::{Header, HttpRequest, HttpResponse, HttpVersion};

#[test]
fn all_custom_action_presets_execute_cleanly_against_sample_http_exchange() {
    let req = HttpRequest {
        method: "POST".into(),
        url: "https://app.example/api/v1/account?id=7".into(),
        version: HttpVersion::Http11,
        headers: vec![
            Header {
                name: "Host".into(),
                value: b"app.example".to_vec(),
            },
            Header {
                name: "Cookie".into(),
                value: b"sid=abc; jwt=eyJhbGciOiJIUzI1NiJ9.eyJ1c2VyIjoiYWxpY2UifQ.sig".to_vec(),
            },
            Header {
                name: "Authorization".into(),
                value: b"Bearer eyJhbGciOiJIUzI1NiJ9.eyJyb2xlIjoiYWRtaW4ifQ.sig".to_vec(),
            },
            Header {
                name: "If-None-Match".into(),
                value: b"\"v1\"".to_vec(),
            },
            Header {
                name: "Sec-Fetch-Site".into(),
                value: b"same-origin".to_vec(),
            },
            Header {
                name: "Content-Type".into(),
                value: b"application/json".to_vec(),
            },
        ],
        body: br#"{"ok":true}"#.to_vec(),
        trailers: Vec::new(),
    };
    let resp = HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: Some(b"OK".to_vec()),
        headers: vec![
            Header {
                name: "Content-Type".into(),
                value: b"text/html".to_vec(),
            },
            Header {
                name: "Location".into(),
                value: b"https://app.example/next".to_vec(),
            },
            Header {
                name: "Set-Cookie".into(),
                value: b"prefs=1; Path=/; HttpOnly".to_vec(),
            },
            Header {
                name: "Access-Control-Allow-Origin".into(),
                value: b"*".to_vec(),
            },
        ],
        body: br#"<a href="/admin">Admin</a><script src="https://cdn.example/app.js"></script>{"api_key":"demo_secret_token"}"#.to_vec(),
        trailers: Vec::new(),
    };

    for preset in CUSTOM_ACTION_PRESETS {
        mio_core::validate_filter_script(preset.script)
            .unwrap_or_else(|e| panic!("preset {} invalid syntax: {e}", preset.label));
        let outcome =
            mio_core::eval_custom_action_with_mutation(preset.script, &req, Some(&resp))
                .unwrap_or_else(|e| panic!("preset {} failed: {e}", preset.label));
        assert!(
            !outcome.output.trim().is_empty(),
            "preset {} should produce output",
            preset.label
        );
        match preset.category {
            CustomActionCategory::MutateRequest => {
                assert!(
                    outcome.updated_request.is_some(),
                    "mutating preset {} should modify request",
                    preset.label
                );
            }
            CustomActionCategory::Inspect => {
                assert!(
                    outcome.updated_request.is_none(),
                    "inspect preset {} must not mutate request",
                    preset.label
                );
            }
        }
    }
}

#[gpui_kit::test]
fn repeater_custom_actions_sidebar_strips_cookie_and_persists_saved_scripts(
    cx: &mut TestAppContext,
) {
    struct ResetPath;
    impl Drop for ResetPath {
        fn drop(&mut self) {
            TEST_CUSTOM_ACTIONS_PATH.with(|cell| *cell.borrow_mut() = None);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let lib_path = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("repeater-custom-actions.json");
    TEST_CUSTOM_ACTIONS_PATH.with(|cell| *cell.borrow_mut() = Some(lib_path.clone()));
    let _reset = ResetPath;

    let mut wt = tab("CustomActions");
    wt.draft.target = "https://target.example".into();
    wt.draft.raw = b"GET /profile HTTP/1.1\r\nHost: target.example\r\nCookie: sid=xyz; role=user\r\nUser-Agent: Test\r\n\r\n".to_vec();
    let tab_id = wt.id;

    let mut f = fixture(cx, vec![wt], 64);
    drain(&mut f.commands);

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.tool = Tool::Repeater;
                view.repeater_custom_actions_open = true;
                view.sync_tab_editors(window, cx);
            });
            window.render_frame(cx);
            assert!(window.find("repeater-custom-actions-sidebar").visible());
            window.click("repeater-custom-action-preset-remove-cookie-run", cx);
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let view = f.view.read(cx);
            let editors = &view.tab_editors[&tab_id];
            let raw = String::from_utf8(editors.request.bytes_lossless(cx)).unwrap();
            assert!(
                !raw.to_ascii_lowercase().contains("cookie:"),
                "Cookie header should be removed from active Repeater draft, got:\n{raw}"
            );
            assert!(raw.contains("User-Agent: Test"));
            assert!(editors.dirty);

            window.click("repeater-custom-action-new", cx);
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("repeater-custom-action-name", cx);
            window.input("添加调试头", cx);
            window.click("repeater-custom-action-save", cx);
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let saved = read_custom_action_library_from_path(&lib_path).unwrap();
            assert_eq!(saved.len(), 1);
            assert_eq!(saved[0].name, "添加调试头");
            assert!(window.find("repeater-custom-actions-saved-list").visible());
            window.click("repeater-custom-action-saved-run-0", cx);
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let view = f.view.read(cx);
            let raw = String::from_utf8(view.tab_editors[&tab_id].request.bytes_lossless(cx))
                .unwrap();
            assert!(
                raw.contains("X-Custom-Test: mio"),
                "custom saved script should mutate active request, got:\n{raw}"
            );
            window.click("repeater-custom-action-saved-del-0", cx);
        })
        .unwrap();
    cx.run_until_parked();

    assert!(
        read_custom_action_library_from_path(&lib_path)
            .unwrap()
            .is_empty()
    );
}
