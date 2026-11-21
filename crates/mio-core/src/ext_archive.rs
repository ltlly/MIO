//! External project archive detection and direct streaming import.

pub(crate) mod direct_import;
pub(crate) mod import;

pub(crate) use direct_import::MAX_REPEATER_TITLE_UNITS;

pub(crate) const HEADER_SIZE: u64 = 72;
pub(crate) const MAGIC: [u8; 4] = [0x66, 0x85, 0x82, 0x80];

/// Returns `true` when `path` is a regular file beginning with the 4-byte external archive magic.
pub fn is_external_archive_file(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 4];
    std::io::Read::read_exact(&mut file, &mut header).is_ok() && header == MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn is_external_archive_file_checks_magic_and_regular_file_type() {
        let dir = tempfile::tempdir().unwrap();

        let valid = dir.path().join("valid.archive");
        let mut bytes = vec![0u8; HEADER_SIZE as usize];
        bytes[..4].copy_from_slice(&MAGIC);
        fs::write(&valid, &bytes).unwrap();
        assert!(is_external_archive_file(&valid));

        let short_magic = dir.path().join("magic-only.archive");
        fs::write(&short_magic, MAGIC).unwrap();
        assert!(is_external_archive_file(&short_magic));

        let truncated = dir.path().join("truncated.archive");
        fs::write(&truncated, &MAGIC[..3]).unwrap();
        assert!(!is_external_archive_file(&truncated));

        let wrong_magic = dir.path().join("wrong.archive");
        fs::write(&wrong_magic, b"SQLite format 3\0").unwrap();
        assert!(!is_external_archive_file(&wrong_magic));

        assert!(!is_external_archive_file(dir.path()));
        assert!(!is_external_archive_file(&dir.path().join("missing.archive")));

        #[cfg(unix)]
        {
            let link = dir.path().join("link.archive");
            std::os::unix::fs::symlink(&valid, &link).unwrap();
            assert!(!is_external_archive_file(&link));
        }
    }
}
