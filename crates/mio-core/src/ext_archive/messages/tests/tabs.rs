use super::*;

#[test]
fn repeater_current_draft_is_not_history_and_pages_keep_nulls_duplicates_and_metadata() {
    let mut builder = Builder::new();
    let current = builder.bytes(b"current draft\0\xff");
    let request = builder.bytes(b"historical request");
    let response = builder.bytes(b"historical response");
    let empty = builder.bytes(&[]);
    let service = builder.object((0, 0), vec![(99, i64::MIN.to_be_bytes().to_vec())]);
    let service = builder.forward(service);
    let metadata = builder.object((0, 17), vec![(87, vec![255])]);
    let first = builder.object(
        (1, 0),
        vec![
            reference(2, request),
            reference(1, metadata),
            reference(3, response),
            reference(0, service),
        ],
    );
    let second = builder.pair(request, empty);
    let addresses = builder.references(&[first as i64, 0, second as i64, first as i64, -1, -1]);
    let history = builder.direct(4, addresses);
    let tab = builder.object(
        (0, 2),
        vec![
            reference(4, current),
            reference(2, history),
            reference(32, 0),
            reference(33, service),
            (99, i64::MIN.to_be_bytes().to_vec()),
        ],
    );
    let null_draft = builder.object((0, 2), vec![reference(2, history), reference(4, 0)]);
    let missing_draft = builder.object((0, 2), vec![reference(2, history)]);
    let empty_draft = builder.object((0, 2), vec![reference(4, empty), reference(2, 0)]);
    let fixture = builder.finish(tab);
    let mut source = fixture.open();
    let mut decoder = Decoder::new(&mut source, MAX_BYTE_BUDGET).unwrap();
    let decoded = decoder.repeater_tab(tab).unwrap();
    let fields = value(&decoded.value);
    assert_eq!(value(&fields.current_request.state).address, current);
    assert_eq!(fields.identity_candidates[0].state, Observation::Null);
    assert_eq!(
        fields.identity_candidates[1].state,
        Observation::Value(service)
    );
    let history = value(&value(&fields.history.state).value);
    assert_eq!(
        (history.logical_len, history.addresses.element_count),
        (4, 6)
    );
    let page = decoder.history_page(history, 0, 2).unwrap();
    assert_eq!(page.len(), 2);
    assert!(matches!(page[1], Observation::Null));
    let pair = value(&value(&page[0]).value);
    assert_eq!(value(&pair.request.state).address, request);
    assert_ne!(value(&pair.request.state).address, current);
    assert_eq!(value(&pair.response.state).address, response);
    let candidate = value(&pair.metadata_candidates[0].state);
    assert_eq!(candidate.forwarded_from.len(), 1);
    assert_eq!(candidate.descriptors[0].id, 99);
    assert_eq!(value(&pair.metadata_candidates[1].state).subtype, 17);
    let next = decoder.history_page(history, 2, 2).unwrap();
    let second = value(&value(&next[0]).value);
    assert_eq!(value(&second.response.state).address, empty);
    assert_eq!(value(&second.response.state).payload_bytes, 0);
    assert_eq!(next[1], page[0]);
    assert!(decoder.history_page(history, 4, 2).unwrap().is_empty());
    let before = decoder.bytes_read();
    assert!(decoder.history_page(history, 0, 257).is_err());
    assert_eq!(decoder.bytes_read(), before);
    assert!(decoder.history_page(history, 5, 1).is_err());
    assert!(decoder.history_page(history, u64::MAX, 1).is_err());
    let null = decoder.repeater_tab(null_draft).unwrap();
    assert_eq!(value(&null.value).current_request.state, Observation::Null);
    assert!(matches!(
        value(&null.value).history.state,
        Observation::Value(_)
    ));
    let missing = decoder.repeater_tab(missing_draft).unwrap();
    assert_eq!(
        value(&missing.value).current_request.state,
        Observation::Missing
    );
    let empty = decoder.repeater_tab(empty_draft).unwrap();
    assert_eq!(
        value(&value(&empty.value).current_request.state).payload_bytes,
        0
    );
    assert_eq!(value(&empty.value).history.state, Observation::Null);
}

#[test]
fn unsupported_schema_and_tags_preserve_structure_without_reading_children() {
    for tag in [(0, 0), (1, 9), (255, 255)] {
        let mut builder = Builder::new();
        let item = builder.object(
            tag,
            vec![(15, (-1_i64).to_be_bytes().to_vec()), integer(127, 5)],
        );
        let fixture = builder.finish(item);
        let mut source = fixture.open();
        let mut decoder = Decoder::new(&mut source, MAX_BYTE_BUDGET).unwrap();
        let decoded = decoder.http_history_item(item).unwrap();
        failed(&decoded.value, io::ErrorKind::Unsupported);
        assert_eq!((decoded.object.object_type, decoded.object.subtype), tag);
        assert_eq!(decoded.object.descriptors.len(), 2);
        assert_eq!(decoder.bytes_read(), 10);
    }
    for schema in [(226, 226), (227, 228), (228, 229)] {
        let mut builder = Builder::new();
        builder.bytes[12..14].copy_from_slice(&u16::try_from(schema.0).unwrap().to_be_bytes());
        builder.bytes[14..16].copy_from_slice(&u16::try_from(schema.1).unwrap().to_be_bytes());
        let item = builder.object((1, 0), vec![reference(15, 0)]);
        let fixture = builder.finish(item);
        let mut source = fixture.open();
        let before = source.bytes_read();
        assert_eq!(
            Decoder::new(&mut source, 4096).err().unwrap().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(source.bytes_read(), before);
    }
}
