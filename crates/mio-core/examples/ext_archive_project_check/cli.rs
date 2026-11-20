use super::*;

pub(crate) fn new_path(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.extension().is_some_and(|ext| ext == "mio"),
        "new targets must end in .mio"
    );
    let name = path.file_name().context("target needs a file name")?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let path = parent
        .canonicalize()
        .context("target parent must already exist")?
        .join(name);
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(suffix);
        match fs::symlink_metadata(Path::new(&candidate)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => bail!("target or SQLite sidecar already exists; nothing will be overwritten"),
            Err(error) => return Err(error).context("cannot check target ownership"),
        }
    }
    Ok(path)
}

pub(crate) fn arguments(args: impl Iterator<Item = OsString>) -> Result<Paths> {
    let args: Vec<_> = args.collect();
    ensure!(args.len() == 3, USAGE);
    let source = PathBuf::from(&args[0]);
    let name = source.file_name().context("source needs a file name")?;
    let parent = source
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Resolve only the parent: canonicalizing the final component would follow a source symlink.
    let source = parent
        .canonicalize()
        .context("cannot resolve source parent")?
        .join(name);
    SourceStamp::read(fs::symlink_metadata(&source).context("source does not exist")?)?;
    let converted = new_path(Path::new(&args[1]))?;
    let saved = new_path(Path::new(&args[2]))?;
    ensure!(
        converted != saved && source != converted && source != saved,
        "all three paths must be distinct"
    );
    Ok(Paths {
        source,
        converted,
        saved,
    })
}

pub(crate) fn phase(output: &mut impl Write, name: &str, started: &mut Instant) -> Result<()> {
    writeln!(
        output,
        "phase={name} wall_ms={}",
        started.elapsed().as_millis()
    )?;
    *started = Instant::now();
    Ok(())
}
