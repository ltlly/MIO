use super::*;
use crate::ext_archive::{ArrayKind, CompactObject};
use std::path::Path;

struct Inspector {
    source: Container,
    byte_budget: u64,
    objects: Vec<ObjectEntry>,
}

impl Inspector {
    fn read<T>(
        &mut self,
        upper_bound: u64,
        read: impl FnOnce(&mut Container) -> io::Result<T>,
    ) -> io::Result<T> {
        let remaining = self.byte_budget.saturating_sub(self.source.bytes_read());
        if upper_bound > remaining {
            return Err(io::Error::other(format!(
                "External catalog byte budget: operation needs at most {upper_bound}, remaining {remaining}"
            )));
        }
        let before = self.source.bytes_read();
        let result = read(&mut self.source);
        debug_assert!(self.source.bytes_read() - before <= upper_bound);
        result
    }

    fn reference(&mut self, object: &CompactObject, id: u8) -> Observation<u64> {
        if !object.descriptors.iter().any(|field| field.id == id) {
            return Observation::Missing;
        }
        match self.read(8, |source| source.field_ref(object, id)) {
            Ok(Some(address)) => Observation::Value(address),
            Ok(None) => Observation::Null,
            Err(error) => Observation::Failed(error.into()),
        }
    }

    fn object(
        &mut self,
        path: &str,
        address: Observation<u64>,
        expected_tag: (u8, u8),
    ) -> Observation<CompactObject> {
        let object = address.and_then(|address| {
            match self.read(COMPACT_READ_BOUND, |source| source.compact_object(address)) {
                Ok(object) => Observation::Value(object),
                Err(error) => Observation::Failed(error.into()),
            }
        });
        self.objects.push(ObjectEntry {
            path: path.to_owned(),
            state: object.clone().map(|object| ObjectSummary {
                object_type: object.object_type,
                subtype: object.subtype,
                fields: object.descriptors,
                forwarding_hops: object.forwarded_from.len(),
                layout_supported: (object.object_type, object.subtype) == expected_tag,
            }),
        });
        object.and_then(|object| {
            if (object.object_type, object.subtype) == expected_tag {
                Observation::Value(object)
            } else {
                Observation::Failed(Failure {
                    kind: io::ErrorKind::Unsupported,
                    message: format!(
                        "Unverified External layout at {path}: type {}, subtype {}",
                        object.object_type, object.subtype
                    ),
                })
            }
        })
    }

    fn child(
        &mut self,
        parent: &Observation<CompactObject>,
        parent_path: &str,
        id: u8,
        expected_tag: (u8, u8),
    ) -> Observation<CompactObject> {
        let address = match parent {
            Observation::Value(parent) => self.reference(parent, id),
            _ => Observation::Unavailable {
                parent: parent_path.to_owned(),
            },
        };
        self.object(&format!("{parent_path}.f{id}"), address, expected_tag)
    }

    fn count(&mut self, object: &CompactObject, id: u8) -> io::Result<u64> {
        let value = self.read(4, |source| source.field_i32(object, id))?;
        u64::try_from(value).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("External collection f{id} has a negative count"),
            )
        })
    }

    fn direct(&mut self, object: &CompactObject) -> io::Result<DirectList> {
        let logical_len = self.count(object, 0)?;
        let address = required(self.reference(object, 1), "f1 backing array")?;
        let addresses = self.read(8, |source| {
            source.array_header(address, ArrayKind::References)
        })?;
        Ok(DirectList {
            logical_len,
            addresses,
        })
    }

    fn collection(
        &mut self,
        object: CompactObject,
        path: &str,
        chunked: bool,
    ) -> io::Result<CollectionSummary> {
        let (source, layout) = if chunked {
            let logical_len = self.count(&object, 0)?;
            let chunk_width = self.count(&object, 1)?;
            let leading_offset = self.count(&object, 3)?;
            let chunk_object = self.child(&Observation::Value(object), path, 2, (0, 0));
            let chunks = self.direct(&required(chunk_object, "f2 chunk list")?)?;
            let layout = Layout::Chunked {
                chunk_width,
                leading_offset,
                active_chunks: chunks.logical_len,
                chunk_capacity: chunks.addresses.element_count,
            };
            let list = ChunkedList {
                logical_len,
                chunk_width,
                leading_offset,
                chunks,
            };
            (CollectionSource::Chunked(list), layout)
        } else {
            let list = self.direct(&object)?;
            let layout = Layout::Direct {
                capacity: list.addresses.element_count,
            };
            (CollectionSource::Direct(list), layout)
        };
        let logical_len = source.logical_len();
        let sampled = logical_len.min(SAMPLE_ITEMS as u64);
        let bound = match &source {
            CollectionSource::Direct(_) => 8 + 8 * sampled,
            // At worst each sampled slot needs its own chunk reference and header.
            CollectionSource::Chunked(_) => 8 + 24 * sampled,
        };
        let sample = self.read(bound, |reader| source.page(reader, 0, SAMPLE_ITEMS));
        Ok(CollectionSummary {
            logical_len,
            layout,
            source,
            sample: match sample {
                Ok(slots) => Observation::Value(
                    slots
                        .into_iter()
                        .map(|slot| {
                            if slot.is_some() {
                                Slot::Present
                            } else {
                                Slot::Null
                            }
                        })
                        .collect(),
                ),
                Err(error) => Observation::Failed(error.into()),
            },
        })
    }
}

fn required<T>(value: Observation<T>, field: &str) -> io::Result<T> {
    match value {
        Observation::Value(value) => Ok(value),
        Observation::Failed(error) => Err(io::Error::new(error.kind, error.message)),
        Observation::Missing => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("External {field} is missing"),
        )),
        Observation::Null => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("External {field} is null, not an empty array"),
        )),
        Observation::Unavailable { .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("External {field} could not be inspected"),
        )),
    }
}

/// Inspect schema-228 collections or the bounded schema-229 settings/tab/root.f1 paths.
/// Missing/null/empty stay distinct; unknown fields retain descriptors, not guessed values.
/// Only 228/228 and 229/229 are accepted. The byte budget includes the 48 read header bytes.
/// Compact-object calls conservatively reserve their maximum before reading, so a very
/// small budget may stop even when a particular object's descriptor table would fit.
pub fn inspect(path: impl AsRef<Path>, byte_budget: u64) -> io::Result<Catalog> {
    inspect_with_unpack(path, byte_budget).map(|(catalog, _)| catalog)
}

pub(crate) fn inspect_with_unpack(
    path: impl AsRef<Path>,
    byte_budget: u64,
) -> io::Result<(Catalog, Observation<bool>)> {
    if !(HEADER_READ_BYTES..=MAX_BYTE_BUDGET).contains(&byte_budget) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "External catalog budget must be between 48 and 262144 bytes",
        ));
    }
    let source = Container::open(path)?;
    let schema_floor = source.header().schema_floor;
    let schema_revision = source.header().schema_revision;
    if !matches!((schema_floor, schema_revision), (228, 228) | (229, 229)) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "External catalog only supports schema 228/228 collections or 229/229 settings/current tabs, found {schema_floor}/{schema_revision}"
            ),
        ));
    }
    let root_address = match source.header().project_root {
        0 => Observation::Null,
        address => Observation::Value(address),
    };
    let mut inspector = Inspector {
        source,
        byte_budget,
        objects: Vec::new(),
    };
    let root = inspector.object("root", root_address, (0, 7));
    let mut collections = Vec::with_capacity(6);
    let mut unpack = Observation::Unavailable {
        parent: "schema 228/228".into(),
    };
    let mut repeater_active_tab = Observation::Unavailable {
        parent: if (schema_floor, schema_revision) == (229, 229) {
            "root.f2"
        } else {
            "schema 228/228"
        }
        .into(),
    };
    if (schema_floor, schema_revision) == (229, 229) {
        let settings = inspector.child(&root, "root", 4, (0, 1));
        let repeater = inspector.child(&settings, "root.f4", 2, (0, 0));
        unpack = match repeater {
            Observation::Value(object) => {
                if object.descriptors.iter().any(|field| field.id == 1) {
                    match inspector.read(1, |source| source.field_bool(&object, 1)) {
                        Ok(value) => Observation::Value(value),
                        Err(error) => Observation::Failed(error.into()),
                    }
                } else {
                    Observation::Missing
                }
            }
            _ => Observation::Unavailable {
                parent: "root.f4.f2".into(),
            },
        };
        // Missing f2 does not expand a settings-only catalog or claim an empty tool.
        if matches!(&root, Observation::Value(object) if object.descriptors.iter().any(|field| field.id == 2))
        {
            let branch = inspector.child(&root, "root", 2, (0, 2));
            let object = inspector.child(&branch, "root.f2", 1, (0, 0));
            let path = "root.f2.f1";
            let state = object.and_then(|object| match inspector.collection(object, path, false) {
                Ok(value) => Observation::Value(value),
                Err(error) => Observation::Failed(error.into()),
            });
            collections.push(CollectionEntry {
                path: path.into(),
                state,
            });
            if matches!(&branch, Observation::Value(object) if object.descriptors.iter().any(|field| field.id == 4))
            {
                let path = "root.f2.f4";
                let object = inspector.child(&branch, "root.f2", 4, (0, 0));
                let state =
                    object.and_then(|object| match inspector.collection(object, path, false) {
                        Ok(value) => Observation::Value(value),
                        Err(error) => Observation::Failed(error.into()),
                    });
                collections.push(CollectionEntry {
                    path: path.into(),
                    state,
                });
            }
            repeater_active_tab = match branch {
                Observation::Value(object) => {
                    match object.descriptors.iter().position(|field| field.id == 0) {
                        None => Observation::Missing,
                        Some(position) => {
                            let field = &object.descriptors[position];
                            if object.descriptors.get(position + 1).and_then(|next| {
                                next.relative_offset.checked_sub(field.relative_offset)
                            }) != Some(4)
                            {
                                Observation::Failed(Failure {
                                    kind: io::ErrorKind::InvalidData,
                                    message:
                                        "External root.f2.f0 is not a verified four-byte integer field"
                                            .into(),
                                })
                            } else {
                                match inspector.read(4, |source| source.field_i32(&object, 0)) {
                                    Ok(value) => Observation::Value(value),
                                    Err(error) => Observation::Failed(error.into()),
                                }
                            }
                        }
                    }
                }
                _ => Observation::Unavailable {
                    parent: "root.f2".into(),
                },
            };
        }
        // An absent branch stays absent in settings-only sources, never four empty directories.
        if matches!(&root, Observation::Value(object) if object.descriptors.iter().any(|field| field.id == 1))
        {
            let branch = inspector.child(&root, "root", 1, (0, 0));
            for id in 0..=3 {
                let path = format!("root.f1.f{id}");
                let object = inspector.child(&branch, "root.f1", id, (0, 0));
                let state =
                    object.and_then(|object| match inspector.collection(object, &path, true) {
                        Ok(value) => Observation::Value(value),
                        Err(error) => Observation::Failed(error.into()),
                    });
                collections.push(CollectionEntry { path, state });
            }
        }
    } else {
        for (branch_id, tag, fields, chunked) in [
            (1, (0, 0), &[0, 1, 2, 3][..], true),
            (2, (0, 2), &[1, 4][..], false),
        ] {
            let branch_path = format!("root.f{branch_id}");
            let branch = inspector.child(&root, "root", branch_id, tag);
            for &id in fields {
                let path = format!("{branch_path}.f{id}");
                let object = inspector.child(&branch, &branch_path, id, (0, 0));
                let state =
                    object.and_then(
                        |object| match inspector.collection(object, &path, chunked) {
                            Ok(value) => Observation::Value(value),
                            Err(error) => Observation::Failed(error.into()),
                        },
                    );
                collections.push(CollectionEntry { path, state });
            }
        }
    }
    Ok((
        Catalog {
            schema_floor,
            schema_revision,
            byte_budget,
            bytes_read: inspector.source.bytes_read(),
            objects: inspector.objects,
            collections,
            repeater_active_tab,
        },
        unpack,
    ))
}
