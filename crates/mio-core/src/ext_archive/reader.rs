use super::*;
use std::io::{Read, Seek, SeekFrom};

impl Reader {
    pub(super) fn new(file: File, boundary: u64) -> Self {
        Self {
            file,
            boundary,
            bytes_read: 0,
            objects_read: 0,
            byte_budget: MAX_READ_BYTES,
            object_budget: MAX_OBJECTS,
        }
    }

    pub(super) fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> io::Result<()> {
        let length = u64::try_from(bytes.len()).map_err(|_| invalid("External read is too large"))?;
        check_range(offset, length, self.boundary)?;
        let total = self
            .bytes_read
            .checked_add(length)
            .filter(|total| *total <= self.byte_budget)
            .ok_or_else(|| invalid("External structure inspection exceeds its byte budget"))?;
        self.bytes_read = total;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(bytes)?;
        Ok(())
    }

    fn object_read(&mut self) -> io::Result<()> {
        if self.objects_read >= self.object_budget {
            return Err(invalid(
                "External structure inspection exceeds its object budget",
            ));
        }
        self.objects_read += 1;
        Ok(())
    }

    pub(super) fn header(&mut self) -> io::Result<Header> {
        if self.boundary < HEADER_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "External outer header is truncated",
            ));
        }
        // The identifier and reserved region at 16..40 is deliberately not read.
        let mut prefix = [0; 16];
        self.read_at(0, &mut prefix)?;
        if prefix[..4] != MAGIC {
            return Err(invalid("File does not have the external container magic"));
        }
        let outer_version = i32::from_be_bytes(prefix[4..8].try_into().unwrap());
        if outer_version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("Unsupported external outer storage version {outer_version}"),
            ));
        }
        let compatibility = u32::from_be_bytes(prefix[8..12].try_into().unwrap());
        if compatibility != COMPATIBILITY {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("Unsupported external container compatibility value {compatibility:#010x}"),
            ));
        }
        let mut layout = [0; 32];
        self.read_at(40, &mut layout)?;
        let physical_address_span = address(&layout[8..16])?;
        if physical_address_span != i64::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Multi-file External address spans are not supported",
            ));
        }
        let allocation_cursor = address(&layout[16..24])?;
        if allocation_cursor < HEADER_SIZE || allocation_cursor > self.boundary {
            return Err(invalid(
                "External allocation cursor is outside the physical file",
            ));
        }
        Ok(Header {
            outer_version,
            compatibility,
            schema_floor: u16::from_be_bytes(prefix[12..14].try_into().unwrap()),
            schema_revision: u16::from_be_bytes(prefix[14..16].try_into().unwrap()),
            metadata_root: address(&layout[..8])?,
            physical_address_span,
            allocation_cursor,
            project_root: address(&layout[24..])?,
        })
    }

    pub(super) fn compact_object(&mut self, mut offset: u64) -> io::Result<CompactObject> {
        let mut forwarded_from: Vec<ForwardingRecord> = Vec::new();
        loop {
            if offset < HEADER_SIZE {
                return Err(invalid(
                    "External object points into the outer header or a null address",
                ));
            }
            if forwarded_from.iter().any(|record| record.address == offset) {
                return Err(invalid("External forwarding records form a cycle"));
            }
            self.object_read()?;
            let mut flags = [0];
            self.read_at(offset, &mut flags)?;
            let tail = offset
                .checked_add(1)
                .ok_or_else(|| invalid("External object offset overflows u64"))?;
            if flags[0] & 1 != 0 {
                if forwarded_from.len() >= MAX_FORWARD_DEPTH {
                    return Err(invalid(
                        "External forwarding depth exceeds the inspection limit",
                    ));
                }
                let mut replacement = [0; 8];
                self.read_at(tail, &mut replacement)?;
                let replacement_address = address(&replacement)?;
                forwarded_from.push(ForwardingRecord {
                    address: offset,
                    flags: flags[0],
                    replacement_address,
                });
                offset = replacement_address;
                continue;
            }
            let mut header = [0; 3];
            self.read_at(tail, &mut header)?;
            let count = usize::from(header[2]);
            if count > MAX_DESCRIPTORS {
                return Err(invalid(
                    "External descriptor count exceeds the inspection limit",
                ));
            }
            let table_length = count
                .checked_mul(3)
                .ok_or_else(|| invalid("External descriptor table length overflows"))?;
            let table_address = offset
                .checked_add(4)
                .ok_or_else(|| invalid("External descriptor table offset overflows u64"))?;
            let mut table = [0; MAX_DESCRIPTORS * 3];
            self.read_at(table_address, &mut table[..table_length])?;
            let mut descriptors = Vec::with_capacity(count);
            let mut seen = [false; 256];
            let mut previous = u16::try_from(4 + table_length - 1)
                .map_err(|_| invalid("External descriptor table is too large"))?;
            for entry in table[..table_length].as_chunks::<3>().0 {
                let id = entry[0];
                if seen[usize::from(id)] {
                    return Err(invalid("External compact object has duplicate field IDs"));
                }
                seen[usize::from(id)] = true;
                let relative_offset = u16::try_from(i16::from_be_bytes([entry[1], entry[2]]))
                    .map_err(|_| invalid("External descriptor has a negative relative offset"))?;
                if relative_offset <= previous {
                    return Err(invalid(
                        "External field offsets overlap the table or are not increasing",
                    ));
                }
                let field_address = offset
                    .checked_add(u64::from(relative_offset))
                    .ok_or_else(|| invalid("External field address overflows u64"))?;
                // Only the start is known: do not invent a width for unknown fields.
                check_range(field_address, 1, self.boundary)?;
                descriptors.push(FieldDescriptor {
                    id,
                    relative_offset,
                });
                previous = relative_offset;
            }
            return Ok(CompactObject {
                address: offset,
                flags: flags[0],
                object_type: header[0],
                subtype: header[1],
                descriptors,
                forwarded_from,
            });
        }
    }

    pub(super) fn field<const N: usize>(
        &mut self,
        object: &CompactObject,
        id: u8,
    ) -> io::Result<[u8; N]> {
        let index = object
            .descriptors
            .iter()
            .position(|field| field.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "External field is absent"))?;
        let relative = u64::from(object.descriptors[index].relative_offset);
        let table_end = object
            .descriptors
            .len()
            .checked_mul(3)
            .and_then(|length| length.checked_add(4))
            .ok_or_else(|| invalid("External descriptor table length overflows"))?;
        if object.address < HEADER_SIZE || relative < table_end as u64 {
            return Err(invalid("External field overlaps its container header"));
        }
        let end = relative
            .checked_add(N as u64)
            .ok_or_else(|| invalid("External field width overflows u64"))?;
        if object
            .descriptors
            .get(index + 1)
            .is_some_and(|next| end > u64::from(next.relative_offset))
        {
            return Err(invalid(
                "External typed field read overlaps the next descriptor",
            ));
        }
        let offset = object
            .address
            .checked_add(relative)
            .ok_or_else(|| invalid("External field address overflows u64"))?;
        let mut bytes = [0; N];
        self.read_at(offset, &mut bytes)?;
        Ok(bytes)
    }

    pub(super) fn reference(&self, bytes: &[u8]) -> io::Result<Option<u64>> {
        let value = address(bytes)?;
        if value == 0 {
            return Ok(None);
        }
        if value < HEADER_SIZE {
            return Err(invalid("External reference points into the outer header"));
        }
        check_range(value, 1, self.boundary)?;
        Ok(Some(value))
    }

    pub(super) fn array_header(&mut self, offset: u64, kind: ArrayKind) -> io::Result<ArrayRecord> {
        if offset < HEADER_SIZE {
            return Err(invalid(
                "External array points into the outer header or a null address",
            ));
        }
        self.object_read()?;
        let mut bytes = [0; 8];
        self.read_at(offset, &mut bytes)?;
        let total_size = u64::try_from(i32::from_be_bytes(bytes[..4].try_into().unwrap()))
            .map_err(|_| invalid("External array has a negative total size"))?;
        let element_count = u64::try_from(i32::from_be_bytes(bytes[4..].try_into().unwrap()))
            .map_err(|_| invalid("External array has a negative element count"))?;
        if total_size < 8 {
            return Err(invalid("External array total size is shorter than its header"));
        }
        check_range(offset, total_size, self.boundary)?;
        // Nine-byte empty reference records were observed in schema 228; UTF-16 remains unverified.
        if element_count == 0 && kind == ArrayKind::Utf16 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Empty UTF-16 External array padding is not verified",
            ));
        }
        let payload_bytes = element_count
            .checked_mul(kind.width())
            .ok_or_else(|| invalid("External array payload length overflows u64"))?;
        let expected = payload_bytes
            .max(1)
            .checked_add(8)
            .ok_or_else(|| invalid("External array total size overflows u64"))?;
        if total_size != expected {
            return Err(invalid(
                "External array size does not match its count and element width",
            ));
        }
        Ok(ArrayRecord {
            address: offset,
            kind,
            total_size,
            element_count,
            payload_bytes,
        })
    }

    pub(super) fn verify_array(&mut self, record: &ArrayRecord) -> io::Result<()> {
        if self.array_header(record.address, record.kind)? != *record {
            return Err(invalid(
                "External array metadata does not match the source record",
            ));
        }
        Ok(())
    }

    pub(super) fn payload(
        &mut self,
        record: &ArrayRecord,
        offset: u64,
        bytes: &mut [u8],
    ) -> io::Result<()> {
        check_range(offset, bytes.len() as u64, record.payload_bytes)?;
        let position = record
            .address
            .checked_add(8)
            .and_then(|start| start.checked_add(offset))
            .ok_or_else(|| invalid("External array payload address overflows u64"))?;
        self.read_at(position, bytes)
    }

    pub(super) fn reference_slots(
        &mut self,
        record: &ArrayRecord,
        start: u64,
        count: usize,
    ) -> io::Result<Vec<Option<u64>>> {
        if record.kind != ArrayKind::References || count > MAX_PAGE_ITEMS {
            return Err(invalid("External reference page kind or size is invalid"));
        }
        check_range(start, count as u64, record.element_count)?;
        let offset = start
            .checked_mul(8)
            .ok_or_else(|| invalid("External reference index overflows u64"))?;
        let mut bytes = vec![0; count * 8];
        self.payload(record, offset, &mut bytes)?;
        bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|slot| self.reference(slot))
            .collect()
    }

    pub(super) fn direct_header(&mut self, list: &DirectList) -> io::Result<()> {
        if list.addresses.kind != ArrayKind::References {
            return Err(invalid("External list does not have a reference array"));
        }
        self.verify_array(&list.addresses)?;
        if list.logical_len > list.addresses.element_count {
            return Err(invalid(
                "External logical list length exceeds its backing capacity",
            ));
        }
        Ok(())
    }

    pub(super) fn chunked_page(
        &mut self,
        list: &ChunkedList,
        start: u64,
        limit: usize,
    ) -> io::Result<Vec<Option<u64>>> {
        let count = page_count(list.logical_len, start, limit, MAX_PAGE_ITEMS)?;
        if list.chunk_width == 0 {
            return Err(invalid("External chunk width is zero"));
        }
        let end = list
            .leading_offset
            .checked_add(list.logical_len)
            .ok_or_else(|| invalid("External chunked logical extent overflows u64"))?;
        let capacity = list
            .chunks
            .logical_len
            .checked_mul(list.chunk_width)
            .ok_or_else(|| invalid("External chunked capacity overflows u64"))?;
        if end > capacity {
            return Err(invalid(
                "External chunked logical extent exceeds its active chunks",
            ));
        }
        self.direct_header(&list.chunks)?;
        let mut result = Vec::with_capacity(count);
        while result.len() < count {
            let physical = list
                .leading_offset
                .checked_add(start)
                .and_then(|position| position.checked_add(result.len() as u64))
                .ok_or_else(|| invalid("External chunked index overflows u64"))?;
            let chunk_index = physical / list.chunk_width;
            let slot = physical % list.chunk_width;
            let chunk_address = self.reference_slots(&list.chunks.addresses, chunk_index, 1)?[0]
                .ok_or_else(|| invalid("External active chunk is absent"))?;
            if chunk_address == list.chunks.addresses.address {
                return Err(invalid("External chunk list refers to itself as a chunk"));
            }
            let chunk = self.array_header(chunk_address, ArrayKind::References)?;
            let take = (list.chunk_width - slot).min((count - result.len()) as u64) as usize;
            result.extend(self.reference_slots(&chunk, slot, take)?);
        }
        Ok(result)
    }
}
