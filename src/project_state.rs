use std::{
    fs::{self, Metadata},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use crate::is_path_redirect;

pub(crate) fn directory(
    root: &Path,
    namespace: &str,
    components: &[&str],
    create: bool,
) -> Result<Option<PathBuf>> {
    let mut dir = root.to_owned();
    for component in [".omp-toolkit", namespace]
        .into_iter()
        .chain(components.iter().copied())
    {
        dir.push(component);
        let mut metadata = checked_metadata(&dir)?;
        if create && metadata.is_none() {
            match fs::create_dir(&dir) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err).with_context(|| format!("create {}", dir.display())),
            }
            metadata = checked_metadata(&dir)?;
        }
        let Some(metadata) = metadata else {
            return Ok(None);
        };
        ensure!(
            metadata.is_dir(),
            "expected a directory at {}",
            dir.display()
        );
    }
    Ok(Some(dir))
}

pub(crate) fn checked_metadata(path: &Path) -> Result<Option<Metadata>> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("inspect {}", path.display())),
    };
    ensure!(
        !is_path_redirect(&metadata),
        "symbolic links are excluded: {}",
        path.display()
    );
    Ok(Some(metadata))
}
