use super::*;

#[test]
fn boolean_fields_accept_only_zero_and_one_with_a_single_byte_budget() {
    for value in 0..=u8::MAX {
        let mut bytes = primitive_bytes();
        put_object(&mut bytes, 192, &[(9, 13), (1, 15), (2, 16)]);
        bytes[207] = value;
        let fixture = Fixture::new(&bytes);
        let mut source = Container::open(&fixture.path).unwrap();
        let object = source.compact_object(192).unwrap();
        let before = source.bytes_read();
        let result = source.field_bool(&object, 1);
        assert_eq!(source.bytes_read() - before, 1);
        match value {
            0 => assert!(!result.unwrap()),
            1 => assert!(result.unwrap()),
            _ => {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert!(error.to_string().contains("boolean"));
            }
        }
        assert_eq!(
            source.field_bool(&object, 99).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    }
}

#[test]
fn boolean_fields_reject_overlap_and_unallocated_last_field_bytes() {
    let mut bytes = primitive_bytes();
    put_object(&mut bytes, 192, &[(1, 10), (2, 11)]);
    bytes[202] = 1;
    put_object(&mut bytes, 1008, &[(1, 15)]);
    bytes[1023] = 0;
    let fixture = Fixture::new(&bytes);
    let mut source = Container::open(&fixture.path).unwrap();
    let mut object = source.compact_object(192).unwrap();
    assert!(source.field_bool(&object, 1).unwrap());
    object.descriptors[1].relative_offset = 10;
    assert!(
        source
            .field_bool(&object, 1)
            .unwrap_err()
            .to_string()
            .contains("overlaps")
    );
    object.descriptors[0].relative_offset = 9;
    assert!(
        source
            .field_bool(&object, 1)
            .unwrap_err()
            .to_string()
            .contains("header")
    );
    let mut last = source.compact_object(1008).unwrap();
    assert!(!source.field_bool(&last, 1).unwrap());
    last.descriptors[0].relative_offset = 16;
    assert!(
        source
            .field_bool(&last, 1)
            .unwrap_err()
            .to_string()
            .contains("boundary")
    );
}

#[test]
fn boolean_fields_follow_verified_forwarding_and_reject_changed_sources() {
    let mut bytes = primitive_bytes();
    put_forward(&mut bytes, 128, 192);
    put_object(&mut bytes, 192, &[(1, 7)]);
    bytes[199] = 1;
    let fixture = Fixture::new(&bytes);
    let mut source = Container::open(&fixture.path).unwrap();
    let object = source.compact_object(128).unwrap();
    assert_eq!(object.forwarded_from.len(), 1);
    assert!(source.field_bool(&object, 1).unwrap());
    File::options()
        .write(true)
        .open(&fixture.path)
        .unwrap()
        .set_len(2049)
        .unwrap();
    let before = source.bytes_read();
    let error = source.field_bool(&object, 1).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("changed"));
    assert_eq!(source.bytes_read(), before);
}

#[test]
fn typed_fields_keep_null_missing_and_invalid_references_distinct() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let object = source.compact_object(192).unwrap();
    assert_eq!(source.field_i32(&object, 10).unwrap(), -42);
    assert_eq!(source.field_ref(&object, 11).unwrap(), Some(72));
    assert!(
        source
            .field_ref(&object, 10)
            .unwrap_err()
            .to_string()
            .contains("overlaps")
    );
    assert_eq!(
        source.field_ref(&object, 99).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    for pointer in [0, -1, 64, 1024, i64::MAX] {
        let mut bytes = primitive_bytes();
        put_i64(&mut bytes, 206, pointer);
        let fixture = Fixture::new(&bytes);
        let mut source = Container::open(&fixture.path).unwrap();
        let object = source.compact_object(192).unwrap();
        let result = source.field_ref(&object, 11);
        if pointer == 0 {
            assert_eq!(result.unwrap(), None);
        } else {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
    }
}

#[test]
fn typed_last_field_cannot_cross_the_allocation_cursor() {
    let mut bytes = primitive_bytes();
    put_object(&mut bytes, 1008, &[(1, 12)]);
    let fixture = Fixture::new(&bytes);
    let mut source = Container::open(&fixture.path).unwrap();
    let object = source.compact_object(1008).unwrap();
    assert!(source.field_i32(&object, 1).is_ok());
    assert_eq!(
        source.field_ref(&object, 1).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}
