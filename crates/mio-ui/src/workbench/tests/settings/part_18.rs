use super::*;

pub(super) fn verify_http_filter_and_user_tls_certificates(
    mut fixture: Fixture,
    cx: &mut TestAppContext,
) {
    // Verify parameterized_only in HTTP history filter dialog
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("http-history-filter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let filter_window = cx.read(|cx| fixture.view.read(cx).http_filter.window.unwrap());
    cx.update_window(filter_window, |_, window, cx| {
        window.render_frame(cx);
        window.click("http-filter-parameterized-only", cx);
        window.click("http-filter-apply-close", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let filter_query = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::QueryHttpHistory { query } => Some(query),
            _ => None,
        })
        .expect("applying parameterized_only filter must enqueue QueryHttpHistory");
    assert!(filter_query.filter.parameterized_only);
    fixture
        .events
        .try_send(Event::HttpHistoryPage {
            query: filter_query,
            result: Ok(mio_core::HttpHistoryResult {
                total: 0,
                rows: Vec::new(),
                selected_rows: Vec::new(),
                selected_ranks: Vec::new(),
                selected_matches: Vec::new(),
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(fixture.view.read(cx).http_filter.applied.parameterized_only);
    });

    let settings = open_settings(&fixture, cx);
    let sample_pem = TLS_TEST_PEM;
    let sample_der = tls_test_certificate();
    let sample_path = std::env::temp_dir().join(format!("mio-settings-ca-{}.cer", Uuid::new_v4()));
    std::fs::write(&sample_path, sample_pem).unwrap();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-network-connections", cx);
        window.render_frame(cx);
        assert!(
            window
                .try_find("settings-upstream-tls-cert-input")
                .is_none()
        );
        assert!(
            window
                .try_find("settings-upstream-tls-system-roots")
                .is_none()
        );
        window.click("settings-network-tls", cx);
        window.render_frame(cx);
        for _ in 0..4 {
            if window.find("settings-tls-ca-add").visible() {
                break;
            }
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-500.))),
                cx,
            );
            window.render_frame(cx);
        }
        assert_eq!(
            window.find("settings-tls-custom-ca-heading").role(),
            Some(gpui_kit::accesskit::Role::Heading)
        );
        assert!(
            window
                .within("settings-tls-custom-ca-section")
                .find_by_label("名称")
                .visible()
        );
        assert_eq!(window.find("settings-tls-ca-remove").disabled(), Some(true));
        window.click("settings-tls-ca-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        Some(vec![sample_path.clone()])
    });
    cx.run_until_parked();
    std::fs::remove_file(&sample_path).unwrap();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetUserTlsCertificates {
            operation_id,
            expected_revision,
            certificates,
        },
    ] = commands.as_slice()
    else {
        panic!("TLS file import must save global user certificates, not workspace settings");
    };
    assert_eq!(*expected_revision, 0);
    assert_eq!(certificates, &vec![sample_der.clone()]);
    let tls_preferences = mio_core::UserPreferences {
        revision: expected_revision + 1,
        custom_ca_ders: certificates.clone(),
        ..mio_core::UserPreferences::default()
    };
    fixture
        .events
        .try_send(Event::UserPreferencesSaved {
            operation_id: *operation_id,
            result: Ok(tls_preferences.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).user_preferences.confirmed.as_ref(),
            Some(&tls_preferences)
        );
        assert!(fixture.view.read(cx).workspace.upstream_tls.is_default());
    });

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("MIO-test-root").visible());
        window.click(("settings-tls-ca-row", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("settings-tls-ca-remove").disabled(), None);
        window.click("settings-tls-ca-remove", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetUserTlsCertificates {
            operation_id,
            expected_revision,
            certificates,
        },
    ] = commands.as_slice()
    else {
        panic!("Remove must save user certificates without writing project settings");
    };
    assert_eq!(*expected_revision, 1);
    assert!(certificates.is_empty());
    fixture
        .events
        .try_send(Event::UserPreferencesSaved {
            operation_id: *operation_id,
            result: Ok(mio_core::UserPreferences {
                revision: 2,
                ..mio_core::UserPreferences::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("MIO-test-root").is_none());
        assert_eq!(window.find("settings-tls-ca-remove").disabled(), Some(true));
        window.click("settings-tls-ca-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());

    let invalid_path =
        std::env::temp_dir().join(format!("mio-settings-invalid-ca-{}.cer", Uuid::new_v4()));
    std::fs::write(
        &invalid_path,
        "-----BEGIN CERTIFICATE-----\nAQIDBA==\n-----END CERTIFICATE-----",
    )
    .unwrap();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-tls-ca-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.simulate_path_prompt_response(|_| Some(vec![invalid_path.clone()]));
    cx.run_until_parked();
    std::fs::remove_file(&invalid_path).unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("settings-tls-error").visible());
    })
    .unwrap();

    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            assert!(view.set_upstream_tls(
                Some(fixture.project),
                mio_core::UpstreamTlsSettings {
                    custom_ca_ders: vec![sample_der.clone()],
                    include_system_roots: false,
                    insecure_skip_verify: false,
                    ..Default::default()
                },
                window,
                cx
            ));
        });
    })
    .unwrap();
    cx.run_until_parked();
    let legacy_ws = saved(&mut fixture.commands);
    acknowledge(&fixture, legacy_ws, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..4 {
            if window.find("settings-tls-use-user-settings").visible() {
                break;
            }
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-400.))),
                cx,
            );
            window.render_frame(cx);
        }
        let legacy = window.find("settings-tls-legacy-override");
        assert!(legacy.visible());
        assert!(legacy.label().unwrap().contains("MIO-test-root"));
        window.click("settings-tls-use-user-settings", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cleared_ws = saved(&mut fixture.commands);
    assert!(cleared_ws.upstream_tls.is_default());
    acknowledge(&fixture, cleared_ws, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("settings-tls-legacy-override").is_none());
    })
    .unwrap();
    cx.simulate_keystrokes(settings, "escape");
}

#[gpui_kit::test]
fn settings_tls_custom_protocols_and_ciphers_persist_and_validate(cx: &mut TestAppContext) {
    let initial = tab("自定义 TLS 协议与密码套件测试");
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);

    let settings = open_settings(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-network-tls", cx);
        window.render_frame(cx);
        window.click("settings-tls-proto-custom", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let custom_ws = saved(&mut fixture.commands);
    assert_eq!(
        custom_ws.upstream_tls.protocol_mode,
        mio_core::TlsProtocolMode::Custom
    );
    assert_eq!(
        custom_ws.upstream_tls.custom_protocols,
        vec!["TLSv1.3", "TLSv1.2"]
    );
    assert!(custom_ws.upstream_tls.custom_cipher_suites.is_empty());
    acknowledge(&fixture, custom_ws, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("settings-tls-custom-panel").visible());
        window.click("settings-tls-custom-proto-tls13", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let tls12_ws = saved(&mut fixture.commands);
    assert_eq!(tls12_ws.upstream_tls.custom_protocols, vec!["TLSv1.2"]);
    acknowledge(&fixture, tls12_ws, cx);

    // Incompatible TLS 1.3 cipher when only TLS 1.2 is enabled must show inline error and not save.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-tls-custom-cipher-TLS13_CHACHA20_POLY1305_SHA256", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let alert = window.find("settings-tls-negotiation-error");
        assert!(alert.visible());
        assert!(alert.label().unwrap().contains("不兼容"));
        for _ in 0..4 {
            if window
                .find("settings-tls-custom-cipher-TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256")
                .visible()
            {
                break;
            }
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-200.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click(
            "settings-tls-custom-cipher-TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let cipher_ws = saved(&mut fixture.commands);
    assert_eq!(
        cipher_ws.upstream_tls.custom_cipher_suites,
        vec!["TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256"]
    );
    acknowledge(&fixture, cipher_ws, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("settings-tls-negotiation-error").is_none());
        for _ in 0..4 {
            if window.find("settings-tls-custom-cipher-reset").visible() {
                break;
            }
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(200.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-tls-custom-cipher-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let reset_ws = saved(&mut fixture.commands);
    assert!(reset_ws.upstream_tls.custom_cipher_suites.is_empty());
    acknowledge(&fixture, reset_ws, cx);

    // Disabling the last remaining protocol version must be rejected with inline error.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..4 {
            if window.find("settings-tls-custom-proto-tls12").visible() {
                break;
            }
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(200.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-tls-custom-proto-tls12", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("settings-tls-negotiation-error").visible());
    })
    .unwrap();
    cx.simulate_keystrokes(settings, "escape");
}
