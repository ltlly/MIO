use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ext_archive) struct RepeaterGroup229 {
    pub name: Field<Decoded<String>>,
    pub color: Field<u8>,
    pub expanded: Field<bool>,
}

pub(in crate::ext_archive) fn repeater_group_229(
    source: &mut Container,
    address: u64,
    byte_budget: u64,
    cancel: &CancellationToken,
) -> io::Result<Decoded<RepeaterGroup229>> {
    let mut reader = decoder_229(source, byte_budget, cancel)?;
    let decoded = reader.mapped(address, (0, 1), "Repeater group", |reader, object| {
        Ok(RepeaterGroup229 {
            name: reader.field(object, 0, |reader, address| {
                reader.mapped(address, (0, 0), "Repeater group name", name)
            }),
            color: reader.primitive(object, 1, |reader| {
                reader.read(1, |source| source.field_u8(object, 1))
            }),
            expanded: reader.primitive(object, 2, |reader| {
                reader.read(1, |source| source.field_bool(object, 2))
            }),
        })
    })?;
    reader.check_cancel()?;
    Ok(decoded)
}

fn name(reader: &mut Decoder<'_>, object: &CompactObject) -> io::Result<String> {
    let address = reader
        .read(8, |source| source.field_ref(object, 0))?
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Null Repeater group name array")
        })?;
    let array = reader.read(8, |source| source.array_header(address, ArrayKind::Utf16))?;
    if !(1..=4096).contains(&array.element_count) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Repeater group name exceeds the verified 1..=4096 UTF-16 unit bound",
        ));
    }
    let units = reader.read(8 + array.payload_bytes, |source| {
        source.utf16_page(&array, 0, array.element_count as usize)
    })?;
    let name = String::from_utf16(&units).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid UTF-16 Repeater group name",
        )
    })?;
    if name.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Repeater group name exceeds 4096 UTF-8 bytes",
        ));
    }
    Ok(name)
}

pub(super) fn group_reference(
    reader: &mut Decoder<'_>,
    tab: &CompactObject,
) -> Field<CompactObject> {
    reader.field(tab, 32, |reader, address| {
        let group = reader.read(COMPACT_READ_BOUND, |source| source.compact_object(address))?;
        if (group.object_type, group.subtype) != (0, 1) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unverified Repeater group reference tag",
            ));
        }
        Ok(group)
    })
}
