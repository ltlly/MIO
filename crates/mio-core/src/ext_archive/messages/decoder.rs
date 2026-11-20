use super::*;

pub(super) fn decoder_229<'a>(
    source: &'a mut Container,
    byte_budget: u64,
    cancel: &'a CancellationToken,
) -> io::Result<Decoder<'a>> {
    if !(1..=MAX_BYTE_BUDGET).contains(&byte_budget) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "External message metadata budget must be 1..=262144 bytes",
        ));
    }
    if (
        source.header().schema_floor,
        source.header().schema_revision,
    ) != (229, 229)
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "外部归档 Repeater mapping only supports schema 229/229",
        ));
    }
    Ok(Decoder {
        started_at: source.bytes_read(),
        source,
        byte_budget,
        cancel: Some(cancel),
    })
}

impl<'a> Decoder<'a> {
    pub fn new(source: &'a mut Container, byte_budget: u64) -> io::Result<Self> {
        if !(1..=MAX_BYTE_BUDGET).contains(&byte_budget) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "External message metadata budget must be 1..=262144 bytes",
            ));
        }
        if (
            source.header().schema_floor,
            source.header().schema_revision,
        ) != (228, 228)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "External message mapping only supports the observed schema 228/228",
            ));
        }
        Ok(Self {
            started_at: source.bytes_read(),
            source,
            byte_budget,
            cancel: None,
        })
    }

    /// Actual additional metadata bytes, including failed reads, not reserved upper bounds.
    pub fn bytes_read(&self) -> u64 {
        self.source.bytes_read() - self.started_at
    }

    /// Only for items reached through root.f1.f0/f1. Tags are not globally unique:
    /// Repeater pairs also use (1,0), and root.f1.f2/f3 remain unmapped categories.
    pub fn http_history_item(&mut self, address: u64) -> io::Result<Decoded<HttpHistoryItem>> {
        self.mapped(address, (1, 0), "HTTP history item", |reader, object| {
            Ok(HttpHistoryItem {
                request: reader.bytes(object, 15),
                unverified_request_references: [16, 17]
                    .map(|id| reader.field(object, id, |_, address| Ok(address))),
                response: reader.bytes(object, 18),
                unverified_response_references: [19, 20]
                    .map(|id| reader.field(object, id, |_, address| Ok(address))),
            })
        })
    }

    /// Only for tab items reached through root.f2.f1; does not enumerate history entries.
    pub fn repeater_tab(&mut self, address: u64) -> io::Result<Decoded<RepeaterTab>> {
        self.mapped(address, (0, 2), "Repeater tab", |reader, object| {
            Ok(RepeaterTab {
                current_request: reader.bytes(object, 4),
                history: reader.field(object, 2, |reader, address| {
                    reader.mapped(address, (0, 0), "Repeater history list", Self::direct)
                }),
                identity_candidates: [32, 33]
                    .map(|id| reader.field(object, id, |_, address| Ok(address))),
            })
        })
    }

    /// Only for entries reached through a decoded Repeater tab's f2 history collection.
    pub fn repeater_pair(&mut self, address: u64) -> io::Result<Decoded<RepeaterPair>> {
        self.mapped(address, (1, 0), "Repeater pair", |reader, object| {
            Ok(RepeaterPair {
                request: reader.bytes(object, 2),
                response: reader.bytes(object, 3),
                metadata_candidates: [0, 1].map(|id| {
                    reader.field(object, id, |reader, address| {
                        reader.read(COMPACT_READ_BOUND, |source| source.compact_object(address))
                    })
                }),
            })
        })
    }

    /// <=256 original positions, including nulls, duplicate references and per-item failures.
    /// The logical length, not backing capacity, bounds the page. No payloads are read.
    #[cfg(test)]
    pub fn history_page(
        &mut self,
        history: &DirectList,
        start: u64,
        limit: usize,
    ) -> io::Result<Vec<Observation<Decoded<RepeaterPair>>>> {
        if limit > MAX_PAGE_ITEMS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "External message history page exceeds 256 items",
            ));
        }
        let addresses = self.read(8 + 8 * limit as u64, |source| {
            source.direct_page(history, start, limit)
        })?;
        Ok(addresses
            .into_iter()
            .map(|address| match address {
                None => Observation::Null,
                Some(address) => observed(self.repeater_pair(address)),
            })
            .collect())
    }

    pub(super) fn read<T>(
        &mut self,
        upper_bound: u64,
        read: impl FnOnce(&mut Container) -> io::Result<T>,
    ) -> io::Result<T> {
        self.check_cancel()?;
        if upper_bound > self.byte_budget.saturating_sub(self.bytes_read()) {
            return Err(io::Error::other(
                "External message metadata byte budget exhausted",
            ));
        }
        let before = self.source.bytes_read();
        let result = read(self.source);
        debug_assert!(self.source.bytes_read() - before <= upper_bound);
        self.check_cancel()?;
        result
    }

    pub(super) fn check_cancel(&self) -> io::Result<()> {
        if self.cancel.is_some_and(|cancel| cancel.is_cancelled()) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "外部归档 Repeater metadata read cancelled",
            ));
        }
        Ok(())
    }

    pub(super) fn mapped<T>(
        &mut self,
        address: u64,
        tag: (u8, u8),
        context: &str,
        decode: impl FnOnce(&mut Self, &CompactObject) -> io::Result<T>,
    ) -> io::Result<Decoded<T>> {
        let object = self.read(COMPACT_READ_BOUND, |source| source.compact_object(address))?;
        let value = if (object.object_type, object.subtype) == tag {
            observed(decode(self, &object))
        } else {
            Observation::Failed(Failure {
                kind: io::ErrorKind::Unsupported,
                message: format!(
                    "Unverified External {context} tag: type {}, subtype {}",
                    object.object_type, object.subtype
                ),
            })
        };
        Ok(Decoded { object, value })
    }

    pub(super) fn field<T>(
        &mut self,
        object: &CompactObject,
        id: u8,
        decode: impl FnOnce(&mut Self, u64) -> io::Result<T>,
    ) -> Field<T> {
        let state = if !object.descriptors.iter().any(|field| field.id == id) {
            Observation::Missing
        } else {
            match self.read(8, |source| source.field_ref(object, id)) {
                Ok(None) => Observation::Null,
                Ok(Some(address)) => observed(decode(self, address)),
                Err(error) => Observation::Failed(error.into()),
            }
        };
        Field { id, state }
    }

    pub(super) fn bytes(&mut self, object: &CompactObject, id: u8) -> Field<ArrayRecord> {
        self.field(object, id, |reader, address| {
            reader.read(8, |source| source.array_header(address, ArrayKind::Bytes))
        })
    }

    pub(super) fn primitive<T>(
        &mut self,
        object: &CompactObject,
        id: u8,
        decode: impl FnOnce(&mut Self) -> io::Result<T>,
    ) -> Field<T> {
        let state = if object.descriptors.iter().any(|field| field.id == id) {
            observed(decode(self))
        } else {
            Observation::Missing
        };
        Field { id, state }
    }

    pub(super) fn service_229(&mut self, address: u64) -> io::Result<Decoded<RepeaterService229>> {
        self.mapped(address, (0, 0), "Repeater service", |reader, service| {
            Ok(RepeaterService229 {
                host: reader.field(service, 0, |reader, address| {
                    reader.mapped(address, (0, 0), "Repeater host", Decoder::host_229)
                }),
                port: reader.primitive(service, 1, |reader| {
                    let port = reader.read(4, |source| source.field_i32(service, 1))?;
                    if !(1..=65535).contains(&port) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "External service port is outside 1..=65535",
                        ));
                    }
                    Ok(port)
                }),
                tls: reader.primitive(service, 2, |reader| {
                    reader.read(1, |source| source.field_bool(service, 2))
                }),
            })
        })
    }

    fn host_229(&mut self, object: &CompactObject) -> io::Result<String> {
        let address = self
            .read(8, |source| source.field_ref(object, 0))?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Null External service host array")
            })?;
        let array = self.read(8, |source| source.array_header(address, ArrayKind::Utf16))?;
        if !(1..=4096).contains(&array.element_count) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "External service host is outside the 1..=4096 UTF-16 unit bound",
            ));
        }
        let units = self.read(8 + array.payload_bytes, |source| {
            source.utf16_page(&array, 0, array.element_count as usize)
        })?;
        let host = String::from_utf16(&units).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid UTF-16 in External service host",
            )
        })?;
        let authority = if host.contains(':') {
            host.parse::<std::net::Ipv6Addr>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "External service host colon is not a bare IPv6 address",
                )
            })?;
            format!("[{host}]")
        } else {
            host.clone()
        };
        crate::validate_target_origin(&format!("http://{authority}:1")).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid or oversized External service host",
            )
        })?;
        Ok(host)
    }

    pub(super) fn chunked_title_229(&mut self, object: &CompactObject) -> io::Result<String> {
        let length = self.read(4, |source| source.field_i32(object, 0))?;
        if length == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Empty 外部归档 Repeater titles are not verified",
            ));
        }
        if !(1..=MAX_REPEATER_TITLE_UNITS as i32).contains(&length) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "External title logical length is outside 1..=4096 UTF-16 units",
            ));
        }
        let wrapper = self
            .read(8, |source| source.field_ref(object, 3))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Null External title wrapper"))?;
        let wrapper = self.read(COMPACT_READ_BOUND, |source| source.compact_object(wrapper))?;
        if (wrapper.object_type, wrapper.subtype) != (0, 0) || wrapper.address == object.address {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unverified External title wrapper layout",
            ));
        }
        let length = length as usize;
        let count = length.div_ceil(REPEATER_TITLE_CHUNK_UNITS);
        if self.read(4, |source| source.field_i32(&wrapper, 0))? != count as i32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "External title active chunk count does not match its logical length",
            ));
        }
        let backing = self
            .read(8, |source| source.field_ref(&wrapper, 1))?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Null External title references")
            })?;
        let backing = self.read(8, |source| {
            source.array_header(backing, ArrayKind::References)
        })?;
        if backing.element_count < count as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "External title reference capacity is smaller than its active chunk count",
            ));
        }
        let chunks = self.read(8 + count as u64 * 8, |source| {
            source.references_page(&backing, 0, count)
        })?;
        let mut units = Vec::with_capacity(length);
        for chunk in chunks {
            let chunk = chunk.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Null active External title chunk")
            })?;
            let chunk = self.read(8, |source| source.array_header(chunk, ArrayKind::Utf16))?;
            if chunk.element_count != REPEATER_TITLE_CHUNK_UNITS as u64 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Unverified External title UTF-16 chunk capacity",
                ));
            }
            let active = (length - units.len()).min(REPEATER_TITLE_CHUNK_UNITS);
            units.extend(self.read(8 + 2 * active as u64, |source| {
                source.utf16_page(&chunk, 0, active)
            })?);
        }
        // A surrogate pair can straddle two chunks; decode only the complete logical title.
        let title = String::from_utf16(&units).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid UTF-16 in 外部归档 Repeater title",
            )
        })?;
        if title.len() > MAX_REPEATER_TITLE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "外部归档 Repeater title exceeds the 4096 UTF-8 byte limit",
            ));
        }
        Ok(title)
    }

    pub(super) fn direct(&mut self, object: &CompactObject) -> io::Result<DirectList> {
        let logical_len = self.read(4, |source| source.field_i32(object, 0))?;
        let logical_len = u64::try_from(logical_len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "Negative External history length")
        })?;
        let address = self
            .read(8, |source| source.field_ref(object, 1))?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Null External history backing array",
                )
            })?;
        let addresses = self.read(8, |source| {
            source.array_header(address, ArrayKind::References)
        })?;
        let history = DirectList {
            logical_len,
            addresses,
        };
        // Validate logical length/capacity without following a history item or inactive slot.
        self.read(8, |source| source.direct_page(&history, 0, 0))?;
        Ok(history)
    }
}
