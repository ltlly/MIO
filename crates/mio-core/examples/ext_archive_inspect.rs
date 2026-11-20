//! Read-only metadata catalog, not a External importer or a project-content exporter.

use mio_core::ext_archive::catalog::{self, Catalog, Layout, Observation, Slot};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

const USAGE: &str = "usage: ext_archive_inspect <source.archive> [--budget-bytes 48..262144]";

fn arguments(mut args: impl Iterator<Item = OsString>) -> io::Result<(PathBuf, u64)> {
    let path = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, USAGE))?;
    let budget = match args.next() {
        None => catalog::MAX_BYTE_BUDGET,
        Some(option) if option == "--budget-bytes" => args
            .next()
            .and_then(|value| value.to_str().and_then(|value| value.parse::<u64>().ok()))
            .filter(|budget| (48..=catalog::MAX_BYTE_BUDGET).contains(budget))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, USAGE))?,
        Some(_) => return Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE)),
    };
    if args.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE));
    }
    Ok((PathBuf::from(path), budget))
}

fn state<T>(output: &mut impl Write, value: &Observation<T>) -> io::Result<()> {
    match value {
        Observation::Missing => writeln!(output, "state=missing"),
        Observation::Null => writeln!(output, "state=null"),
        Observation::Unavailable { parent } => {
            writeln!(output, "state=unavailable parent={parent}")
        }
        Observation::Failed(error) => writeln!(
            output,
            "state=failed kind={:?} reason={}",
            error.kind, error.message
        ),
        Observation::Value(_) => unreachable!("metadata values have a dedicated renderer"),
    }
}

fn write_report(output: &mut impl Write, report: &Catalog) -> io::Result<()> {
    writeln!(
        output,
        "inspection=structure-only schema={}/{} byte_budget={} bytes_read={} body_bytes_read=0",
        report.schema_floor, report.schema_revision, report.byte_budget, report.bytes_read
    )?;
    for object in &report.objects {
        write!(output, "{} ", object.path)?;
        if let Observation::Value(object) = &object.state {
            write!(
                output,
                "type={} subtype={} layout_supported={} forwarding_hops={} field_ids=",
                object.object_type, object.subtype, object.layout_supported, object.forwarding_hops
            )?;
            for (index, field) in object.fields.iter().enumerate() {
                if index > 0 {
                    write!(output, ",")?;
                }
                write!(output, "f{}", field.id)?;
            }
            writeln!(output)?;
        } else {
            state(output, &object.state)?;
        }
    }
    for collection in &report.collections {
        write!(output, "{} ", collection.path)?;
        if let Observation::Value(collection) = &collection.state {
            write!(output, "f0.logical_len={} ", collection.logical_len)?;
            match &collection.layout {
                Layout::Direct { capacity } => {
                    writeln!(output, "layout=direct f1.capacity={capacity}")?
                }
                Layout::Chunked {
                    chunk_width,
                    leading_offset,
                    active_chunks,
                    chunk_capacity,
                } => writeln!(
                    output,
                    "layout=chunked f1.chunk_width={chunk_width} f3.leading_offset={leading_offset} f2.f0.active_chunks={active_chunks} f2.f1.capacity={chunk_capacity}"
                )?,
            }
            write!(output, "  sample ")?;
            if let Observation::Value(slots) = &collection.sample {
                let nulls = slots.iter().filter(|slot| **slot == Slot::Null).count();
                writeln!(
                    output,
                    "slots={} present={} null={nulls}",
                    slots.len(),
                    slots.len() - nulls
                )?;
            } else {
                state(output, &collection.sample)?;
            }
        } else {
            state(output, &collection.state)?;
        }
    }
    writeln!(
        output,
        "root.f1.business_category=unverified root.f2.business_category=Repeater-candidate full_project_compatibility=unverified"
    )
}

fn run() -> io::Result<ExitCode> {
    let (path, budget) = arguments(std::env::args_os().skip(1))?;
    let report = catalog::inspect(path, budget)?;
    write_report(&mut io::stdout().lock(), &report)?;
    Ok(if report.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("inspection failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_budget_is_explicit_bounded_and_rejects_extra_arguments() {
        let parse = |args: &[&str]| arguments(args.iter().map(OsString::from));
        assert_eq!(
            parse(&["synthetic.archive"]).unwrap().1,
            catalog::MAX_BYTE_BUDGET
        );
        assert_eq!(
            parse(&["synthetic.archive", "--budget-bytes", "1024"])
                .unwrap()
                .1,
            1024
        );
        for args in [
            &[][..],
            &["synthetic.archive", "--unknown"][..],
            &["synthetic.archive", "--budget-bytes"][..],
            &["synthetic.archive", "--budget-bytes", "47"][..],
            &["synthetic.archive", "--budget-bytes", "262145"][..],
            &["synthetic.archive", "--budget-bytes", "not-a-number"][..],
            &["synthetic.archive", "--budget-bytes", "64", "extra"][..],
        ] {
            assert_eq!(parse(args).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn output_contains_only_structural_metadata_and_not_field_offsets() {
        use mio_core::ext_archive::{
            FieldDescriptor,
            catalog::{ObjectEntry, ObjectSummary},
        };
        let report = Catalog {
            repeater_active_tab: Observation::Unavailable {
                parent: "schema 228/228".into(),
            },
            schema_floor: 228,
            schema_revision: 228,
            byte_budget: 1024,
            bytes_read: 48,
            objects: vec![ObjectEntry {
                path: "root".into(),
                state: Observation::Value(ObjectSummary {
                    object_type: 0,
                    subtype: 7,
                    fields: vec![FieldDescriptor {
                        id: 99,
                        relative_offset: 12345,
                    }],
                    forwarding_hops: 0,
                    layout_supported: true,
                }),
            }],
            collections: vec![],
        };
        let mut output = Vec::new();
        write_report(&mut output, &report).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("field_ids=f99"));
        assert!(output.contains("body_bytes_read=0"));
        assert!(output.contains("full_project_compatibility=unverified"));
        assert!(!output.contains("12345"));
        assert!(!output.contains("address"));
    }
}
