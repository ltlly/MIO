use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, TestAppContext};
use mio_core::{OastInteraction, OastPayloadToken};

use super::*;
use crate::workbench::tests::{drain, fixture};

fn click(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn http_interaction(_project: mio_core::ProjectKey, token: &str) -> OastInteraction {
    OastInteraction {
        id: Uuid::new_v4(),
        token: Some(token.to_owned()),
        protocol: OastProtocol::Http,
        peer_addr: "127.0.0.1:64001".into(),
        summary: format!("GET /oast/{token} HTTP/1.1"),
        raw_request: format!("GET /oast/{token} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").into_bytes(),
        observed_unix_ms: 1,
    }
}

fn dns_interaction(_project: mio_core::ProjectKey, token: &str) -> OastInteraction {
    OastInteraction {
        id: Uuid::new_v4(),
        token: Some(token.to_owned()),
        protocol: OastProtocol::Dns,
        peer_addr: "127.0.0.1:64002".into(),
        summary: format!("A {token}.oast.local"),
        raw_request: vec![
            0x9a, 0xbc, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ],
        observed_unix_ms: 2,
    }
}

#[gpui_kit::test]
fn copy_http_generates_payloads_and_writes_system_clipboard(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-collaborator", cx);
    let drained = drain(&mut f.commands);
    let [query] = drained.as_slice() else {
        panic!("expected QueryStatus on first activation");
    };
    let Command::Collaborator(CollaboratorCommand::QueryStatus { project }) = query else {
        panic!("expected Collaborator QueryStatus");
    };
    assert_eq!(*project, f.project);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("collaborator-onboarding").is_some());
        window.click("collaborator-get-started", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dismissed_cmds = drain(&mut f.commands);
    assert!(matches!(
        dismissed_cmds.as_slice(),
        [Command::SetUserWorkbenchPreferences { misc, .. }]
            if misc.collaborator_onboarding_dismissed
    ));

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("collaborator-count", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("3", cx);
        window.click("collaborator-label", cx);
        window.press(crate::kbd!("a"), cx);
        window.input(" SSRF probe ", cx);
        window.render_frame(cx);
        window.click("collaborator-copy-http", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::Collaborator(CollaboratorCommand::GeneratePayloads {
            project,
            count,
            label,
        }),
    ] = commands.as_slice()
    else {
        panic!("expected GeneratePayloads, got {} commands", commands.len());
    };
    assert_eq!(*project, f.project);
    assert_eq!((*count, label.as_str()), (3, "SSRF probe"));

    let payloads: Vec<OastPayloadToken> = (0..3)
        .map(|index| OastPayloadToken {
            token: format!("{index:016x}"),
            label: "SSRF probe".into(),
            http_callback_url: format!("http://127.0.0.1:54321/oast/{index:016x}"),
            dns_qname: format!("{index:016x}.oast.local"),
            created_unix_ms: 3,
        })
        .collect();
    let expected_http: Vec<String> = payloads
        .iter()
        .map(|payload| payload.http_callback_url.clone())
        .collect();
    f.events
        .try_send(Event::Collaborator(CollaboratorEvent::PayloadsGenerated {
            project: f.project,
            payloads: payloads.clone(),
        }))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("collaborator-payload-preview").label().unwrap(),
            expected_http.join("\n")
        );
    })
    .unwrap();
    cx.read(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(expected_http.join("\n"))
        );
        let pane = &f.view.read(cx).collaborator_pane;
        assert_eq!(
            pane.latest_payload_preview.as_deref(),
            Some(expected_http.join("\n").as_str())
        );
        assert_eq!(pane.status.issued_tokens.len(), 3);
        assert!(pane.notice.as_deref().unwrap().contains("HTTP 回调 URL"));
    });
}

#[gpui_kit::test]
fn interactions_filter_by_protocol_and_load_raw_packets(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    click(f.window, "tool-collaborator", cx);
    let _ = drain(&mut f.commands);
    let http = http_interaction(f.project, "1111111111111111");
    let dns = dns_interaction(f.project, "2222222222222222");
    let dns_id = dns.id;
    f.events
        .try_send(Event::Collaborator(
            CollaboratorEvent::InteractionObserved {
                project: f.project,
                interaction: Box::new(http.clone()),
            },
        ))
        .unwrap();
    f.events
        .try_send(Event::Collaborator(
            CollaboratorEvent::InteractionObserved {
                project: f.project,
                interaction: Box::new(dns.clone()),
            },
        ))
        .unwrap();
    cx.run_until_parked();

    click(f.window, "collaborator-filter-dns", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(SharedString::from(format!(
                    "collaborator-interaction-{}",
                    http.id
                )))
                .is_none()
        );
        assert!(
            window
                .try_find(SharedString::from(format!(
                    "collaborator-interaction-{dns_id}"
                )))
                .is_some()
        );
        window.click(
            SharedString::from(format!("collaborator-interaction-{dns_id}")),
            cx,
        );
    })
    .unwrap();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let (selected, view) = {
            let pane = &f.view.read(cx).collaborator_pane;
            (pane.selected_interaction, pane.raw_editor.view())
        };
        assert_eq!(selected, Some(dns_id));
        assert_eq!(view, MessageView::Hex);
        let bytes = f.view.update(cx, |v, cx| {
            v.collaborator_pane.raw_editor.bytes(window, cx).unwrap()
        });
        assert_eq!(bytes, dns.raw_request);
    })
    .unwrap();

    click(f.window, "collaborator-filter-all", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(
            SharedString::from(format!("collaborator-interaction-{}", http.id)),
            cx,
        );
        window.render_frame(cx);
        let (selected, view, raw) = {
            let pane = &f.view.read(cx).collaborator_pane;
            (
                pane.selected_interaction,
                pane.raw_editor.view(),
                pane.raw_editor.state().read(cx).value().to_string(),
            )
        };
        assert_eq!(selected, Some(http.id));
        assert_eq!(view, MessageView::Raw);
        let bytes = f.view.update(cx, |v, cx| {
            v.collaborator_pane.raw_editor.bytes(window, cx).unwrap()
        });
        assert_eq!(bytes, http.raw_request);
        assert!(raw.contains("GET /oast/1111111111111111"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn config_submits_poll_interval_and_copies_interactions_json(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    click(f.window, "tool-collaborator", cx);
    let _ = drain(&mut f.commands);

    let http = http_interaction(f.project, "abcd1234ef567890");
    f.events
        .try_send(Event::Collaborator(
            CollaboratorEvent::InteractionObserved {
                project: f.project,
                interaction: Box::new(http),
            },
        ))
        .unwrap();
    cx.run_until_parked();

    click(f.window, "collaborator-copy-json", cx);
    cx.read(|cx| {
        let clip = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("JSON copied");
        assert!(clip.contains("abcd1234ef567890"));
    });

    click(f.window, "collaborator-toggle-config", cx);
    click(f.window, "collaborator-self-hosted-enabled", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("collaborator-polling-url", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("http://127.0.0.1:19999/poll", cx);
        window.click("collaborator-poll-interval", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("750", cx);
        window.render_frame(cx);
        window.click("collaborator-apply-config", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cmds = drain(&mut f.commands);
    assert!(matches!(
        cmds.as_slice(),
        [Command::Collaborator(CollaboratorCommand::Start { config, .. })]
            if config.enabled && config.poll_interval_ms == 750
    ));
}

#[gpui_kit::test]
fn onboarding_renders_ext_welcome_without_inspector_and_dismisses_to_workspace(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-collaborator", cx);
    let _ = drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("collaborator-onboarding").is_some());
        assert!(window.try_find("collaborator-onboarding-radar").is_some());
        assert!(window.try_find("collaborator-onboarding-title").is_some());
        assert!(window.try_find("collaborator-learn-more").is_some());
        assert!(window.try_find("collaborator-get-started").is_some());
        assert!(window.try_find("collaborator-dont-show-again").is_some());
        assert!(window.try_find("collaborator-dismiss-onboarding").is_some());
        assert!(window.try_find("inspector-toggle").is_none());
        window.click("collaborator-dismiss-onboarding", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("collaborator-onboarding").is_none());
        assert!(window.try_find("collaborator-table-header").is_some());
        assert!(window.try_find("collaborator-copy-http").is_some());
        assert!(window.try_find("collaborator-raw-editor-pretty").is_some());
        assert!(window.try_find("inspector-toggle").is_none());
    })
    .unwrap();
}
