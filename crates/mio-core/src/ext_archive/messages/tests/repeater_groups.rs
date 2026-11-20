use super::*;
use tokio_util::sync::CancellationToken;

fn builder() -> Builder {
    let mut builder = Builder::new();
    builder.bytes[12..14].copy_from_slice(&229_u16.to_be_bytes());
    builder.bytes[14..16].copy_from_slice(&229_u16.to_be_bytes());
    builder
}

fn group(
    builder: &mut Builder,
    units: &[u16],
    color: u8,
    expanded: u8,
    wrapper_tag: (u8, u8),
) -> u64 {
    let bytes: Vec<_> = units.iter().flat_map(|unit| unit.to_be_bytes()).collect();
    let text = builder.array(2, &bytes);
    let name = builder.object(wrapper_tag, vec![reference(0, text)]);
    builder.object(
        (0, 1),
        vec![
            reference(0, name),
            (1, vec![color]),
            (2, vec![expanded]),
            integer(3, i32::MIN),
            (4, i64::MIN.to_be_bytes().to_vec()),
        ],
    )
}

#[test]
fn group_name_color_byte_and_expansion_preserve_unknown_fields_without_reading_them() {
    for (color, expanded) in [(0, 0), (7, 1), (12, 0), (255, 1)] {
        let mut builder = builder();
        let address = group(
            &mut builder,
            &"Alpha组🙂".encode_utf16().collect::<Vec<_>>(),
            color,
            expanded,
            (0, 0),
        );
        let fixture = builder.finish(address);
        let before = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let decoded = repeater_group_229(
            &mut source,
            address,
            MAX_BYTE_BUDGET,
            &CancellationToken::new(),
        )
        .unwrap();
        let fields = value(&decoded.value);
        assert_eq!(value(&value(&fields.name.state).value), "Alpha组🙂");
        assert_eq!(fields.color.state, Observation::Value(color));
        assert_eq!(fields.expanded.state, Observation::Value(expanded != 0));
        assert!(decoded.object.descriptors.iter().any(|field| field.id == 3));
        assert!(decoded.object.descriptors.iter().any(|field| field.id == 4));
        let read_before = source.bytes_read();
        assert_eq!(source.field_u8(&decoded.object, 1).unwrap(), color);
        assert_eq!(source.bytes_read() - read_before, 1);
        assert!(source.field_u8(&decoded.object, 127).is_err());
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
    }
}

#[test]
fn group_decoding_keeps_invalid_name_or_bool_explicit_and_cancellation_bounded() {
    for (name, tag, expanded) in [
        (vec![0xd800], (0, 0), 1),
        (vec![b'A' as u16], (0, 2), 1),
        (vec![b'A' as u16; 4097], (0, 0), 1),
        (vec![b'A' as u16], (0, 0), 2),
    ] {
        let mut builder = builder();
        let address = group(&mut builder, &name, 255, expanded, tag);
        let fixture = builder.finish(address);
        let mut source = fixture.open();
        let decoded = repeater_group_229(
            &mut source,
            address,
            MAX_BYTE_BUDGET,
            &CancellationToken::new(),
        )
        .unwrap();
        let fields = value(&decoded.value);
        assert_eq!(fields.color.state, Observation::Value(255));
        if expanded == 2 {
            failed(&fields.expanded.state, io::ErrorKind::InvalidData);
        } else {
            assert!(
                !matches!(&fields.name.state, Observation::Value(name) if matches!(name.value, Observation::Value(_)))
            );
        }
        let before = source.bytes_read();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(repeater_group_229(&mut source, address, MAX_BYTE_BUDGET, &cancel).is_err());
        assert_eq!(source.bytes_read(), before);
        assert!(repeater_group_229(&mut source, address, 1, &CancellationToken::new()).is_err());
        assert_eq!(source.bytes_read(), before);
    }
    let mut old = Builder::new();
    let address = group(&mut old, &[b'A' as u16], 7, 1, (0, 0));
    let fixture = old.finish(address);
    let mut source = fixture.open();
    let before = source.bytes_read();
    assert!(
        repeater_group_229(
            &mut source,
            address,
            MAX_BYTE_BUDGET,
            &CancellationToken::new()
        )
        .is_err()
    );
    assert_eq!(source.bytes_read(), before);
}

#[test]
fn group_membership_resolves_forwarding_but_never_follows_name_or_uuid_payloads() {
    for tag in [(0, 1), (0, 2)] {
        let mut builder = builder();
        let group = builder.object(
            tag,
            vec![
                (0, i64::MIN.to_be_bytes().to_vec()),
                (4, i64::MIN.to_be_bytes().to_vec()),
            ],
        );
        let forwarded = builder.forward(group);
        let tab = builder.object((0, 2), vec![reference(32, forwarded)]);
        let fixture = builder.finish(tab);
        let decoded = repeater_current_229(
            &mut fixture.open(),
            tab,
            MAX_BYTE_BUDGET,
            &CancellationToken::new(),
        )
        .unwrap();
        let fields = value(&decoded.value);
        if tag == (0, 1) {
            let group_value = value(&fields.group.state);
            assert_eq!(group_value.address, group);
            assert_eq!(group_value.forwarded_from.len(), 1);
        } else {
            failed(&fields.group.state, io::ErrorKind::Unsupported);
        }
    }
}
