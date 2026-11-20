use super::*;
use std::path::Path;

impl Container {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        #[cfg(not(unix))]
        if let Ok(meta) = std::fs::symlink_metadata(path)
            && !meta.is_file()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "External inspection requires a regular file",
            ));
        }
        // The path may be replaced before open; inspect the same nonblocking fd that we read.
        let mut opts = File::options();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NONBLOCK);
        }
        let file = opts.open(path)?;
        let original = file.metadata()?;
        if !original.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "External inspection requires a regular file",
            ));
        }
        let mut reader = Reader::new(file, original.len());
        let header = reader.header()?;
        reader.boundary = header.allocation_cursor;
        let source = Self {
            bytes_read: reader.bytes_read,
            objects_read: reader.objects_read,
            reader,
            header,
            original,
        };
        source.check_unchanged()?;
        Ok(source)
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Cumulative requested bytes, including headers and failed reads, across bounded operations.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    fn check_unchanged(&self) -> io::Result<()> {
        let current = self.reader.file.metadata()?;
        if self.original.len() != current.len()
            || self.original.modified()? != current.modified()?
        {
            return Err(invalid("External source changed during structure inspection"));
        }
        Ok(())
    }

    fn operation<T>(
        &mut self,
        bytes: u64,
        objects: usize,
        read: impl FnOnce(&mut Reader) -> io::Result<T>,
    ) -> io::Result<T> {
        self.check_unchanged()?;
        self.reader.bytes_read = 0;
        self.reader.objects_read = 0;
        self.reader.byte_budget = bytes;
        self.reader.object_budget = objects;
        let result = read(&mut self.reader);
        self.bytes_read = self
            .bytes_read
            .checked_add(self.reader.bytes_read)
            .ok_or_else(|| invalid("External read counter overflows u64"))?;
        self.objects_read = self
            .objects_read
            .checked_add(self.reader.objects_read)
            .ok_or_else(|| invalid("External object counter overflows"))?;
        self.check_unchanged()?;
        result
    }

    pub fn compact_object(&mut self, address: u64) -> io::Result<CompactObject> {
        self.operation(MAX_READ_BYTES, MAX_OBJECTS, |reader| {
            reader.compact_object(address)
        })
    }

    /// The caller must establish this field's type; a descriptor alone is not a type tag.
    pub fn field_i32(&mut self, object: &CompactObject, id: u8) -> io::Result<i32> {
        self.operation(4, 0, |reader| {
            reader.field(object, id).map(i32::from_be_bytes)
        })
    }

    /// The caller must establish this field's signed eight-byte scalar layout.
    pub fn field_i64(&mut self, object: &CompactObject, id: u8) -> io::Result<i64> {
        self.operation(8, 0, |reader| {
            reader.field(object, id).map(i64::from_be_bytes)
        })
    }

    /// The caller must establish that this descriptor is an unsigned one-byte field.
    pub fn field_u8(&mut self, object: &CompactObject, id: u8) -> io::Result<u8> {
        self.operation(1, 0, |reader| {
            reader.field::<1>(object, id).map(|[value]| value)
        })
    }

    /// The caller must establish the one-byte boolean layout; only 0 and 1 are valid.
    pub fn field_bool(&mut self, object: &CompactObject, id: u8) -> io::Result<bool> {
        self.operation(1, 0, |reader| match reader.field::<1>(object, id)? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(invalid("External boolean is neither 0 nor 1")),
        })
    }

    /// Missing fields are errors; only the encoded zero address becomes `None`.
    pub fn field_ref(&mut self, object: &CompactObject, id: u8) -> io::Result<Option<u64>> {
        self.operation(8, 0, |reader| {
            let bytes = reader.field::<8>(object, id)?;
            reader.reference(&bytes)
        })
    }

    pub fn array_header(&mut self, address: u64, kind: ArrayKind) -> io::Result<ArrayRecord> {
        self.operation(8, 1, |reader| reader.array_header(address, kind))
    }

    pub fn read_payload(
        &mut self,
        record: &ArrayRecord,
        offset: u64,
        output: &mut [u8],
    ) -> io::Result<()> {
        if output.len() > MAX_PAYLOAD_BYTES {
            return Err(invalid("External payload read exceeds the 64 KiB window"));
        }
        self.operation(MAX_PAYLOAD_BYTES as u64 + 8, 1, |reader| {
            reader.verify_array(record)?;
            reader.payload(record, offset, output)
        })
    }

    pub fn references_page(
        &mut self,
        record: &ArrayRecord,
        start: u64,
        limit: usize,
    ) -> io::Result<Vec<Option<u64>>> {
        let count = page_count(record.element_count, start, limit, MAX_PAGE_ITEMS)?;
        self.operation(MAX_READ_BYTES, 1, |reader| {
            reader.verify_array(record)?;
            reader.reference_slots(record, start, count)
        })
    }

    /// Raw UTF-16 code units are preserved, including unpaired surrogates.
    pub fn utf16_page(
        &mut self,
        record: &ArrayRecord,
        start: u64,
        limit: usize,
    ) -> io::Result<Vec<u16>> {
        if record.kind != ArrayKind::Utf16 {
            return Err(invalid("External record is not a UTF-16 array"));
        }
        let count = page_count(record.element_count, start, limit, MAX_PAYLOAD_BYTES / 2)?;
        let offset = start
            .checked_mul(2)
            .ok_or_else(|| invalid("External UTF-16 index overflows u64"))?;
        self.operation(MAX_PAYLOAD_BYTES as u64 + 8, 1, |reader| {
            reader.verify_array(record)?;
            let mut bytes = vec![0; count * 2];
            reader.payload(record, offset, &mut bytes)?;
            Ok(bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|unit| u16::from_be_bytes([unit[0], unit[1]]))
                .collect())
        })
    }

    pub fn direct_page(
        &mut self,
        list: &DirectList,
        start: u64,
        limit: usize,
    ) -> io::Result<Vec<Option<u64>>> {
        let count = page_count(list.logical_len, start, limit, MAX_PAGE_ITEMS)?;
        self.operation(MAX_READ_BYTES, 1, |reader| {
            reader.direct_header(list)?;
            reader.reference_slots(&list.addresses, start, count)
        })
    }

    pub fn chunked_page(
        &mut self,
        list: &ChunkedList,
        start: u64,
        limit: usize,
    ) -> io::Result<Vec<Option<u64>>> {
        self.operation(MAX_PAYLOAD_BYTES as u64, MAX_PAGE_ITEMS + 1, |reader| {
            reader.chunked_page(list, start, limit)
        })
    }
}

/// Inspect an inactive regular file without reading field values or modifying the source.
/// Unknown schema/type/field IDs remain numeric. This does not validate their semantics.
pub fn inspect(path: impl AsRef<Path>) -> io::Result<Structure> {
    let mut source = Container::open(path)?;
    let header = source.header.clone();
    let (metadata_root, project_root) =
        source.operation(MAX_READ_BYTES, MAX_OBJECTS, |reader| {
            let metadata = (header.metadata_root != 0)
                .then(|| reader.compact_object(header.metadata_root))
                .transpose()?;
            let project = (header.project_root != 0)
                .then(|| reader.compact_object(header.project_root))
                .transpose()?;
            Ok((metadata, project))
        })?;
    Ok(Structure {
        file_size: source.original.len(),
        header,
        metadata_root,
        project_root,
        bytes_read: source.bytes_read,
        objects_read: source.objects_read,
    })
}
