use super::*;

#[test]
fn history_keeps_failed_and_unsupported_items_and_rejects_malformed_collection_metadata() {
    let mut builder = Builder::new();
    let request = builder.bytes(b"synthetic");
    let pair = builder.pair(request, 0);
    let unsupported = builder.object((8, 6), vec![(2, (-1_i64).to_be_bytes().to_vec())]);
    let invalid = builder.raw(&[0, 1, 0, 2, 2, 0, 10, 2, 0, 18]);
    let addresses = builder.references(&[
        pair as i64,
        0,
        invalid as i64,
        unsupported as i64,
        pair as i64,
    ]);
    let history = builder.direct(5, addresses);
    let tab = builder.object((0, 2), vec![reference(2, history), reference(4, request)]);
    let oversized = builder.direct(6, addresses);
    let bad_tab = builder.object((0, 2), vec![reference(2, oversized), reference(4, request)]);
    let null_backing = builder.direct(0, 0);
    let null_tab = builder.object((0, 2), vec![reference(2, null_backing)]);
    let negative = builder.direct(-1, addresses);
    let negative_tab = builder.object((0, 2), vec![reference(2, negative)]);
    let unknown_list = builder.object((0, 8), vec![reference(1, 0)]);
    let unknown_tab = builder.object((0, 2), vec![reference(2, unknown_list)]);
    let fixture = builder.finish(tab);
    let mut source = fixture.open();
    let mut decoder = Decoder::new(&mut source, MAX_BYTE_BUDGET).unwrap();
    let decoded = decoder.repeater_tab(tab).unwrap();
    let history = value(&value(&value(&decoded.value).history.state).value);
    let page = decoder.history_page(history, 0, 5).unwrap();
    assert_eq!(page.len(), 5);
    assert!(matches!(page[1], Observation::Null));
    failed(&page[2], io::ErrorKind::InvalidData);
    failed(&value(&page[3]).value, io::ErrorKind::Unsupported);
    assert_eq!(value(&page[3]).object.descriptors[0].id, 2);
    assert_eq!(page[0], page[4]);
    for tab in [bad_tab, null_tab, negative_tab] {
        let decoded = decoder.repeater_tab(tab).unwrap();
        failed(
            &value(&value(&decoded.value).history.state).value,
            io::ErrorKind::InvalidData,
        );
    }
    let unknown = decoder.repeater_tab(unknown_tab).unwrap();
    let history = value(&value(&unknown.value).history.state);
    failed(&history.value, io::ErrorKind::Unsupported);
    assert_eq!(history.object.subtype, 8);
}

#[test]
fn metadata_budget_is_cumulative_preflighted_and_can_fail_one_slot_without_erasing_others() {
    let mut builder = Builder::new();
    let request = builder.bytes(b"bounded");
    let fields = (0..120)
        .map(|id| reference(id, if matches!(id, 15 | 16) { request } else { 0 }))
        .collect();
    let mut item = builder.object((1, 0), fields);
    for _ in 0..16 {
        item = builder.forward(item);
    }
    let fixture = builder.finish(item);
    let mut source = fixture.open();
    for budget in [0, MAX_BYTE_BUDGET + 1, u64::MAX] {
        assert_eq!(
            Decoder::new(&mut source, budget).err().unwrap().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    {
        let mut decoder = Decoder::new(&mut source, COMPACT_READ_BOUND - 1).unwrap();
        assert!(decoder.http_history_item(item).is_err());
        assert_eq!(decoder.bytes_read(), 0);
    }
    let mut decoder = Decoder::new(&mut source, COMPACT_READ_BOUND).unwrap();
    let decoded = decoder.http_history_item(item).unwrap();
    assert_eq!(decoded.object.forwarded_from.len(), 16);
    let fields = value(&decoded.value);
    assert_eq!(value(&fields.request.state).payload_bytes, 7);
    assert_eq!(
        fields.unverified_request_references[0].state,
        Observation::Value(request)
    );
    failed(
        &fields.unverified_request_references[1].state,
        io::ErrorKind::Other,
    );
    failed(&fields.response.state, io::ErrorKind::Other);
    for field in &fields.unverified_response_references {
        failed(&field.state, io::ErrorKind::Other);
    }
    assert_eq!(decoder.bytes_read(), COMPACT_READ_BOUND);
    assert!(decoder.http_history_item(item).is_err());
    assert_eq!(decoder.bytes_read(), COMPACT_READ_BOUND);
}
