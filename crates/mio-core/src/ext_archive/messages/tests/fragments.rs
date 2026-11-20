use super::*;
use crate::ext_archive::records::{self, Record};
use std::io::Write;

#[test]
fn explicit_fragments_preserve_all_bytes_and_enforce_window_and_range_bounds() {
    let payload: Vec<u8> = (0..=255).collect();
    let mut builder = Builder::new();
    let bytes = builder.bytes(&payload);
    let item = builder.object((1, 0), vec![reference(15, bytes)]);
    let fixture = builder.finish(item);
    let mut source = fixture.open();
    let array = {
        let mut decoder = Decoder::new(&mut source, MAX_BYTE_BUDGET).unwrap();
        let decoded = decoder.http_history_item(item).unwrap();
        assert_eq!(decoder.bytes_read(), 7 + 8 + 8);
        value(&value(&decoded.value).request.state).clone()
    };
    let record = Record::Array(array);
    let mut output = [0; 256];
    records::read_fragment(&mut source, &record, 0, &mut output[..127]).unwrap();
    records::read_fragment(&mut source, &record, 127, &mut output[127..]).unwrap();
    assert_eq!(output.as_slice(), payload);
    records::read_fragment(&mut source, &record, 256, &mut []).unwrap();
    assert!(records::read_fragment(&mut source, &record, 256, &mut [0]).is_err());
    assert!(records::read_fragment(&mut source, &record, u64::MAX, &mut [0]).is_err());
    let before = source.bytes_read();
    assert!(records::read_fragment(&mut source, &record, 0, &mut vec![0; 65537]).is_err());
    assert_eq!(source.bytes_read(), before);
}

#[test]
fn changed_source_is_rejected_for_decode_and_later_fragments_without_reading_stale_bytes() {
    let mut builder = Builder::new();
    let bytes = builder.bytes(b"unchanged original");
    let item = builder.object((1, 0), vec![reference(15, bytes)]);
    let fixture = builder.finish(item);
    let mut source = fixture.open();
    let record = {
        let mut decoder = Decoder::new(&mut source, MAX_BYTE_BUDGET).unwrap();
        let decoded = decoder.http_history_item(item).unwrap();
        let record = Record::Array(value(&value(&decoded.value).request.state).clone());
        fs::OpenOptions::new()
            .append(true)
            .open(&fixture.path)
            .unwrap()
            .write_all(&[0])
            .unwrap();
        let before = decoder.bytes_read();
        let error = decoder.http_history_item(item).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("changed"));
        assert_eq!(decoder.bytes_read(), before);
        record
    };
    let before = source.bytes_read();
    assert!(
        records::read_fragment(&mut source, &record, 0, &mut [0])
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    assert_eq!(source.bytes_read(), before);
}
