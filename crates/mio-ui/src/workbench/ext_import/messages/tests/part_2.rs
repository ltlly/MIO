use super::*;

#[gpui_kit::test]
fn archived_binary_fragments_are_lazy_readonly_and_reject_late_or_mismatched_reads(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, revision) = open(&mut fixture, cx);
    let first = summary(source, 0, 5);
    let second = summary(source, 1, FRAGMENT_BYTES as u64 + 2);
    page(
        &fixture,
        source,
        revision,
        None,
        0,
        2,
        vec![first.clone(), second.clone()],
    );
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "list results never load bodies"
    );
    select(&fixture, 0, cx);
    let first_read = read(&mut fixture);
    assert_eq!(
        first_read.3, 5,
        "short messages must request their exact length"
    );
    select(&fixture, 1, cx);
    let second_read = read(&mut fixture);
    assert_eq!(second_read.1, second.id);
    assert_eq!(second_read.3, FRAGMENT_BYTES);
    let mut bytes = vec![b'a'; FRAGMENT_BYTES];
    bytes[FRAGMENT_BYTES - 1] = 0xe4;
    fragment(&fixture, second_read, second.clone(), bytes);
    fragment(&fixture, first_read, first, b"first".to_vec());
    fixture
        .events
        .try_send(Event::ExtMessage {
            project: fixture.project,
            selection_revision: first_read.0,
            message_id: first_read.1,
            offset: 0,
            result: Err("late old failure".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.detail.as_ref().unwrap().summary.id, second.id);
        assert_eq!(
            messages.detail.as_ref().unwrap().bytes.len(),
            FRAGMENT_BYTES
        );
        assert_eq!(messages.payload.view(), MessageView::Hex);
        assert!(messages.detail_error.is_none());
    });
    click(&fixture, "ext-message-raw", cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.ext_import.messages.as_ref().unwrap().payload.view(),
            MessageView::Hex
        );
        assert_eq!(
            view.error.as_deref(),
            Some("消息含非 UTF-8 字节，请使用 Hex 视图查看原始字节。")
        );
    });
    click(&fixture, "ext-fragment-next", cx);
    let next = read(&mut fixture);
    assert_eq!(next.2, FRAGMENT_BYTES as u64);
    assert_eq!(
        next.3, 2,
        "the last read must not extend past the archive range"
    );
    let mut mismatched = second.clone();
    mismatched.source_id = Uuid::new_v4();
    fragment(&fixture, next, mismatched, vec![0xb8, 0xad]);
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert!(messages.detail.is_none());
        assert!(messages.detail_error.as_ref().unwrap().contains("不一致"));
    });
    click(&fixture, "ext-fragment-retry", cx);
    let retry = read(&mut fixture);
    assert!(retry.0 > next.0);
    assert_eq!(retry.3, 2);
    fragment(&fixture, retry, second, vec![0xb8, 0xad]);
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let editor = fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .payload
            .hex
            .clone();
        assert_eq!(editor.read(cx).value().as_ref(), "b8 ad");
        window
            .within(("hex-grid-view", editor.entity_id()))
            .click(("hex-byte-cell", 0usize), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("00", cx);
        assert_eq!(editor.read(cx).value().as_ref(), "b8 ad");
        window.click("ext-fragment-next", cx);
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "the final fragment has no next page"
    );
    click(&fixture, "ext-fragment-previous", cx);
    assert_eq!(read(&mut fixture).2, 0);
}
