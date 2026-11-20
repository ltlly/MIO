use super::*;

const MAX_NOTE_UNITS: usize = 32_768;
const MAX_NOTE_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ext_archive) struct HttpAnnotations229 {
    pub note: Field<Decoded<HttpNote229>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ext_archive) struct HttpNote229 {
    pub backing: Field<ArrayRecord>,
    pub length: Field<i32>,
    pub text: Observation<String>,
}

pub(super) fn annotations(
    reader: &mut Decoder<'_>,
    item: &CompactObject,
) -> Field<Decoded<HttpAnnotations229>> {
    reader.field(item, 27, |reader, address| {
        reader.mapped(address, (0, 0), "HTTP annotations", |reader, parent| {
            Ok(HttpAnnotations229 {
                note: reader.field(parent, 2, |reader, address| {
                    reader.mapped(address, (0, 0), "HTTP note", note)
                }),
            })
        })
    })
}

fn note(reader: &mut Decoder<'_>, object: &CompactObject) -> io::Result<HttpNote229> {
    let backing = reader.field(object, 1, |reader, address| {
        reader.read(8, |source| source.array_header(address, ArrayKind::Utf16))
    });
    let length = reader.primitive(object, 2, |reader| {
        reader.read(4, |source| source.field_i32(object, 2))
    });
    let text = match (&backing.state, &length.state) {
        (Observation::Value(backing), Observation::Value(length)) => {
            observed(text_prefix(reader, backing, *length))
        }
        _ => Observation::Unavailable {
            parent: "HTTP note backing or logical length".into(),
        },
    };
    Ok(HttpNote229 {
        backing,
        length,
        text,
    })
}

fn text_prefix(reader: &mut Decoder<'_>, backing: &ArrayRecord, length: i32) -> io::Result<String> {
    let length = usize::try_from(length)
        .ok()
        .filter(|length| *length <= MAX_NOTE_UNITS && (*length as u64) <= backing.element_count)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "HTTP note logical length is negative, exceeds backing capacity, or exceeds 32768 UTF-16 units"))?;
    let units = reader.read(8 + 2 * length as u64, |source| {
        source.utf16_page(backing, 0, length)
    })?;
    let text = String::from_utf16(&units).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP note contains invalid UTF-16",
        )
    })?;
    if text.len() > MAX_NOTE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP note exceeds 65536 UTF-8 bytes; text was not truncated",
        ));
    }
    Ok(text)
}
