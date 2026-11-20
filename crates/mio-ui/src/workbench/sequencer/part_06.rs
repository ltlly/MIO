#[cfg(test)]
mod tests {
    use super::super::*;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AnyWindowHandle, ClipboardItem, EntityInputHandler, TestAppContext};
    use mio_core::sequencer::analyze_tokens;

    use crate::editing::COMPOSITION_PENDING;
    use crate::workbench::tests::{drain, fixture};

    fn click(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn xorshift64_star(state: &mut u64) -> u64 {
        *state ^= *state >> 12;
        *state ^= *state << 25;
        *state ^= *state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    #[gpui_kit::test]
    fn live_capture_flow_loads_draft_emits_start_tracks_progress_and_triggers_analysis(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);

        let raw = b"POST /auth/login HTTP/1.1\r\nHost: 127.0.0.1:18080\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nuser=alice";
        let draft = Draft {
            revision: 3,
            target: "http://127.0.0.1:18080/auth/login".into(),
            raw: raw.to_vec(),
            version: HttpVersion::Http2,
        };

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.send_draft_to_sequencer(draft.clone(), window, cx);
            });
            window.render_frame(cx);
            assert!(f.view.read(cx).tool == Tool::Sequencer);
            assert_eq!(f.view.read(cx).sequencer_pane.page, SequencerPage::Live);
            assert_eq!(
                window.find("sequencer-target-url").value(),
                Some("http://127.0.0.1:18080/auth/login")
            );
            assert_eq!(
                f.view.read(cx).sequencer_pane.request_version,
                HttpVersion::Http2
            );
            assert!(window.try_find("sequencer-capture-metrics").is_none());
        })
        .unwrap();

        click(f.window, "sequencer-loc-cookie", cx);
        click(f.window, "sequencer-settings", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("sequencer-cookie-name", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("sid_token", cx);
            window.click("sequencer-target-samples", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("10", cx);
            window.click("sequencer-concurrency", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("3", cx);
        })
        .unwrap();
        cx.run_until_parked();

        click(f.window, "sequencer-settings", cx);
        click(f.window, "sequencer-start-capture", cx);
        let commands = drain(&mut f.commands);
        let [
            Command::Sequencer(SequencerCommand::StartLiveCapture {
                project,
                run_id,
                config,
                cancel,
            }),
        ] = commands.as_slice()
        else {
            panic!(
                "expected single StartLiveCapture command, count={}",
                commands.len()
            );
        };
        assert_eq!(*project, f.project);
        assert!(!cancel.is_cancelled());
        assert_eq!(config.draft.target, "http://127.0.0.1:18080/auth/login");
        assert_eq!(config.draft.version, HttpVersion::Http2);
        assert_eq!(
            config.location,
            SequencerTokenLocation::Cookie {
                name: "sid_token".into(),
            }
        );
        assert_eq!((config.target_samples, config.concurrency), (10, 3));
        let run_id = *run_id;

        f.events
            .try_send(Event::Sequencer(SequencerEvent::CaptureStarted {
                project: f.project,
                run_id,
                target_samples: 10,
            }))
            .unwrap();
        f.events
            .try_send(Event::Sequencer(SequencerEvent::CaptureProgress {
                project: f.project,
                run_id,
                requests_sent: 6,
                tokens_collected: 5,
                extraction_misses: 1,
                request_errors: 0,
                latest_samples: vec!["tok_001".into(), "tok_002".into()],
            }))
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("sequencer-stop-capture").disabled(), None);
            assert_eq!(
                window.find("sequencer-analyze-captured").disabled(),
                Some(true)
            );
            let metrics_el = window.find("sequencer-capture-metrics");
            let metrics = metrics_el.label().unwrap();
            assert!(metrics.contains("已发送 6") && metrics.contains("已提取 5"));
            assert_eq!(
                f.view
                    .read(cx)
                    .sequencer_pane
                    .preview_samples
                    .read(cx)
                    .value()
                    .as_ref(),
                "tok_001\ntok_002"
            );
        })
        .unwrap();

        let captured: Vec<String> = (1..=10).map(|idx| format!("sid_val_{idx:04}")).collect();
        f.events
            .try_send(Event::Sequencer(SequencerEvent::CaptureFinished {
                project: f.project,
                run_id,
                requests_sent: 11,
                tokens_collected: 10,
                extraction_misses: 1,
                request_errors: 0,
                samples: captured.clone(),
                stopped_early: false,
                error: None,
            }))
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("sequencer-stop-capture").disabled(), Some(true));
            assert_eq!(window.find("sequencer-analyze-captured").disabled(), None);
        })
        .unwrap();

        click(f.window, "sequencer-analyze-captured", cx);
        let commands = drain(&mut f.commands);
        let [
            Command::Sequencer(SequencerCommand::AnalyzeSamples {
                project: analyze_project,
                samples,
                ..
            }),
        ] = commands.as_slice()
        else {
            panic!("expected AnalyzeSamples command after capture finished");
        };
        assert_eq!(*analyze_project, f.project);
        assert_eq!(samples, &captured);
    }

    #[gpui_kit::test]
    fn manual_load_guards_ime_and_renders_analysis_report_with_honest_fips_states(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.tool = Tool::Sequencer;
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();

        click(f.window, "sequencer-tab-manual", cx);

        let manual = cx.read(|cx| f.view.read(cx).sequencer_pane.manual_editor.clone());
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", manual.entity_id()), cx);
            manual.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            });
            window.render_frame(cx);
            window.click("sequencer-analyze-manual", cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).error.as_deref(), Some(COMPOSITION_PENDING));
            assert!(manual.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }));
        })
        .unwrap();
        assert!(
            drain(&mut f.commands).is_empty(),
            "IME composition must block AnalyzeSamples submission"
        );

        let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
        let high_entropy_tokens: Vec<String> = (0..500)
            .map(|_| {
                let hi = xorshift64_star(&mut rng);
                let lo = xorshift64_star(&mut rng);
                format!("{hi:016x}{lo:016x}")
            })
            .collect();
        let clipboard_text = format!("\n\n{}\n\n", high_entropy_tokens.join("\n"));

        cx.update_window(f.window, |_, window, cx| {
            manual.update(cx, |state, cx| {
                state.replace_text_in_range(None, "", window, cx);
                state.set_value("", window, cx);
            });
            cx.write_to_clipboard(ClipboardItem::new_string(clipboard_text));
            window.click("dismiss-error", cx);
            window.render_frame(cx);
            window.click("sequencer-paste-clipboard", cx);
            window.render_frame(cx);
            let stats_el = window.find("sequencer-manual-stats");
            let stats_label = stats_el.label().unwrap();
            assert!(stats_label.contains("有效行数：500"));
            assert!(stats_label.contains("最小长度：32 B"));
            assert!(stats_label.contains("最大长度：32 B"));
        })
        .unwrap();

        click(f.window, "sequencer-analyze-manual", cx);
        let commands = drain(&mut f.commands);
        let [
            Command::Sequencer(SequencerCommand::AnalyzeSamples {
                project,
                operation_id,
                samples,
                ..
            }),
        ] = commands.as_slice()
        else {
            panic!("expected AnalyzeSamples from manual load");
        };
        assert_eq!(*project, f.project);
        assert_eq!(samples, &high_entropy_tokens);
        let operation_id = *operation_id;

        let real_analysis = analyze_tokens(samples).expect("valid tokens");
        assert_eq!(real_analysis.quality, QualityRating::Excellent);
        assert_eq!(real_analysis.bit_analysis.fips, FipsStatus::Pass);
        let expected_entropy = format!("{:.1} bits", real_analysis.effective_entropy_bits);

        f.events
            .try_send(Event::Sequencer(SequencerEvent::AnalysisReady {
                project: f.project,
                operation_id,
                result: Ok(Box::new(real_analysis)),
            }))
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).sequencer_pane.page, SequencerPage::Analysis);
            assert_eq!(
                window.find("sequencer-quality-badge").label(),
                Some("Excellent 极佳")
            );
            assert_eq!(
                window.find("sequencer-effective-entropy").label(),
                Some(format!("有效熵 {expected_entropy}").as_str())
            );
            assert_eq!(
                window.find("sequencer-fips-status").label(),
                Some("Pass (4/4 全部通过)")
            );
            assert!(window.find(("sequencer-char-row", 0_usize)).visible());
            assert!(window.find(("sequencer-bit-row", 0_usize)).visible());
        })
        .unwrap();

        // Also verify InsufficientBits (< 20,000 bits) is reported honestly and never faked as Pass.
        click(f.window, "sequencer-tab-manual", cx);
        click(f.window, "sequencer-clear-samples", cx);
        let short_samples = ["AQIDBA==".to_owned(), "+/8AAA==".to_owned()];
        cx.update_window(f.window, |_, window, cx| {
            manual.update(cx, |state, cx| {
                state.set_value(short_samples.join("\n"), window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        click(f.window, "sequencer-analyze-manual", cx);
        let commands = drain(&mut f.commands);
        let [
            Command::Sequencer(SequencerCommand::AnalyzeSamples {
                operation_id: short_op,
                samples: sent_short,
                ..
            }),
        ] = commands.as_slice()
        else {
            panic!("expected AnalyzeSamples for short Base64 set");
        };
        let short_analysis = analyze_tokens(sent_short).unwrap();
        f.events
            .try_send(Event::Sequencer(SequencerEvent::AnalysisReady {
                project: f.project,
                operation_id: *short_op,
                result: Ok(Box::new(short_analysis)),
            }))
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("sequencer-quality-badge").label(),
                Some("样本不足")
            );
            let fips_el = window.find("sequencer-fips-status");
            let fips_label = fips_el.label().unwrap();
            assert!(
                fips_label.contains("样本比特不足") && fips_label.contains("64 bits < 20000 bits")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn sequencer_sample_export_import_and_base64_predecode_analysis(cx: &mut TestAppContext) {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.tool = Tool::Sequencer;
                cx.notify();
            });
            cx.write_to_clipboard(ClipboardItem::new_string(
                URL_SAFE_NO_PAD.encode([0xde, 0xad, 0xbe, 0xef])
                    + "\n"
                    + URL_SAFE_NO_PAD.encode([0xca, 0xfe, 0xba, 0xbe]).as_str(),
            ));
            window.render_frame(cx);
        })
        .unwrap();

        click(f.window, "sequencer-settings", cx);
        click(f.window, "sequencer-toggle-capture-panel", cx);
        click(f.window, "sequencer-base64-predecode", cx);
        click(f.window, "sequencer-settings", cx);

        click(f.window, "sequencer-append-captured", cx);
        cx.read(|cx| {
            assert_eq!(f.view.read(cx).sequencer_pane.captured_samples.len(), 2);
        });

        cx.update_window(f.window, |_, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string("overwritten".to_owned()));
            window.render_frame(cx);
        })
        .unwrap();
        click(f.window, "sequencer-copy-captured", cx);
        cx.read(|cx| {
            let copied = cx
                .read_from_clipboard()
                .and_then(|i| i.text())
                .expect("copied collected tokens");
            assert_eq!(copied.lines().count(), 2);
        });

        click(f.window, "sequencer-analyze-captured", cx);
        let cmds = drain(&mut f.commands);
        let [Command::Sequencer(SequencerCommand::AnalyzeSamples { samples, .. })] =
            cmds.as_slice()
        else {
            panic!("expected AnalyzeSamples with Base64-decoded hex samples");
        };
        assert_eq!(samples, &["deadbeef".to_owned(), "cafebabe".to_owned()]);
    }

    #[gpui_kit::test]
    fn sequencer_ext_layout_request_list_remove_clear_and_custom_location_configure(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.tool = Tool::Sequencer;
                cx.notify();
            });
            window.render_frame(cx);
            assert!(f.view.read(cx).sequencer_pane.live_requests.is_empty());
            assert!(window.find("sequencer-tab-live").visible());
            assert!(window.find("sequencer-tab-manual").visible());
            assert!(window.try_find("sequencer-tab-analysis").is_none());
            assert!(window.try_find("sequencer-capture-metrics").is_none());
            assert_eq!(window.find("sequencer-start-capture").disabled(), Some(true));
            assert_eq!(window.find("sequencer-request-remove").disabled(), Some(true));
            assert_eq!(window.find("sequencer-request-clear").disabled(), Some(true));
            assert_eq!(window.find("sequencer-loc-cookie").disabled(), Some(true));
            assert_eq!(
                window.find("sequencer-loc-form-field").disabled(),
                Some(true)
            );
            assert_eq!(window.find("sequencer-loc-custom").disabled(), Some(true));
            assert_eq!(
                window.find("sequencer-custom-configure").disabled(),
                Some(true)
            );
        })
        .unwrap();

        let draft_one = Draft {
            revision: 1,
            target: "http://127.0.0.1:8080/session".into(),
            raw: b"GET /session HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n".to_vec(),
            version: HttpVersion::Http11,
        };
        let draft_two = Draft {
            revision: 2,
            target: "https://example.test/token".into(),
            raw: b"POST /token HTTP/1.1\r\nHost: example.test\r\n\r\n".to_vec(),
            version: HttpVersion::Http2,
        };

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.send_draft_to_sequencer(draft_one, window, cx);
                view.send_draft_to_sequencer(draft_two, window, cx);
            });
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).sequencer_pane.live_requests.len(), 2);
            assert_eq!(f.view.read(cx).sequencer_pane.selected_request_seq, Some(2));
            assert_eq!(window.find("sequencer-start-capture").disabled(), None);
            assert_eq!(window.find("sequencer-request-remove").disabled(), None);
            assert_eq!(window.find("sequencer-request-clear").disabled(), None);
            assert_eq!(window.find("sequencer-loc-cookie").disabled(), None);
            assert_eq!(window.find("sequencer-loc-form-field").disabled(), None);
            assert_eq!(window.find("sequencer-loc-custom").disabled(), None);
            assert_eq!(
                window.find("sequencer-target-url").value(),
                Some("https://example.test/token")
            );
            assert!(window.find(("sequencer-request-row", 1_usize)).visible());
            assert!(window.find(("sequencer-request-row", 2_usize)).visible());
        })
        .unwrap();

        click(f.window, "sequencer-custom-configure", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                f.view.read(cx).sequencer_pane.location_kind,
                LocationKind::BodyDelimiter
            );
            assert!(window.find("sequencer-delim-start").visible());
            assert!(window.find("sequencer-delim-end").visible());
        })
        .unwrap();

        click(f.window, "sequencer-loc-regex", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                f.view.read(cx).sequencer_pane.location_kind,
                LocationKind::Regex
            );
            assert!(window.find("sequencer-regex-pattern").visible());
        })
        .unwrap();

        click(f.window, "sequencer-loc-header", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                f.view.read(cx).sequencer_pane.location_kind,
                LocationKind::Header
            );
            assert!(window.find("sequencer-header-name").visible());
        })
        .unwrap();

        click(f.window, "sequencer-loc-form-field", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                f.view.read(cx).sequencer_pane.location_kind,
                LocationKind::FormField
            );
            assert!(window.find("sequencer-form-field-name").visible());
        })
        .unwrap();

        click(f.window, "sequencer-request-remove", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).sequencer_pane.live_requests.len(), 1);
            assert_eq!(f.view.read(cx).sequencer_pane.selected_request_seq, Some(1));
            assert_eq!(
                window.find("sequencer-target-url").value(),
                Some("http://127.0.0.1:8080/session")
            );
        })
        .unwrap();

        click(f.window, "sequencer-request-clear", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(f.view.read(cx).sequencer_pane.live_requests.is_empty());
            assert_eq!(f.view.read(cx).sequencer_pane.selected_request_seq, None);
            assert_eq!(window.find("sequencer-start-capture").disabled(), Some(true));
            assert_eq!(window.find("sequencer-loc-cookie").disabled(), Some(true));
        })
        .unwrap();
    }
}
