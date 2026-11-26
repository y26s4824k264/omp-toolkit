use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell, Semaphore};

use crate::{capture, checked, office_read, preview_text};

const MAX_ENTRIES: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Xls,
    Ppt,
}

impl Format {
    pub(crate) fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "xls" => Some(Self::Xls),
            "ppt" => Some(Self::Ppt),
            _ => None,
        }
    }

    fn extensions(self) -> (&'static str, &'static str) {
        match self {
            Self::Xls => ("xls", "xlsx"),
            Self::Ppt => ("ppt", "pptx"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    format: Format,
    digest: [u8; 32],
}

pub(crate) struct Converted {
    _directory: tempfile::TempDir,
    pub(crate) path: PathBuf,
}

type Entry = Arc<OnceCell<Arc<Converted>>>;

pub(crate) struct Cache {
    entries: Mutex<VecDeque<(Key, Entry)>>,
    workers: Semaphore,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            workers: Semaphore::new(2),
        }
    }
}

impl Cache {
    async fn entry(&self, key: Key) -> Entry {
        let mut entries = self.entries.lock().await;
        entries.retain(|(_, entry)| entry.initialized() || Arc::strong_count(entry) > 1);
        if let Some(index) = entries.iter().position(|(candidate, _)| candidate == &key) {
            let entry = entries.remove(index).expect("entry found under this lock");
            let selected = Arc::clone(&entry.1);
            entries.push_front(entry);
            return selected;
        }
        let selected = Arc::new(OnceCell::new());
        entries.push_front((key, Arc::clone(&selected)));
        selected
    }

    async fn prune(&self) {
        let mut completed = 0;
        self.entries.lock().await.retain(|(_, entry)| {
            if entry.initialized() {
                completed += 1;
                completed <= MAX_ENTRIES
            } else {
                // Keep active conversions available to other callers until their owners finish.
                Arc::strong_count(entry) > 1
            }
        });
    }

    pub(crate) async fn get(&self, path: &Path, format: Format) -> Result<Arc<Converted>> {
        let _worker = self.workers.acquire().await?;
        let source = path.to_owned();
        let digest = tokio::task::spawn_blocking(move || snapshot(&source, io::sink()))
            .await
            .context("join legacy Office snapshot")??;
        let key = Key { format, digest };
        let entry = self.entry(key).await;
        let result = entry
            .get_or_try_init(|| convert(path.to_owned(), key))
            .await
            .map(Arc::clone);
        drop(entry);
        self.prune().await;
        result
    }
}

fn snapshot(path: &Path, mut output: impl Write) -> Result<[u8; 32]> {
    let file = File::open(path).context("open legacy Office file")?;
    let limit = office_read::MAX_FILE_BYTES;
    ensure!(
        file.metadata()?.len() <= limit,
        "Office file exceeds the 64 MiB input limit"
    );
    let mut reader = file.take(limit + 1);
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        ensure!(
            reader.limit() > 0,
            "Office file exceeds the 64 MiB input limit"
        );
        digest.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    Ok(digest.finalize().into())
}

async fn convert(source_path: PathBuf, key: Key) -> Result<Arc<Converted>> {
    let (source, target) = key.format.extensions();
    let (directory, input, command, path) = tokio::task::spawn_blocking(move || {
        let directory = tempfile::tempdir().context("create legacy Office conversion directory")?;
        let input = directory.path().join(format!("source.{source}"));
        ensure!(
            snapshot(&source_path, File::create(&input)?)? == key.digest,
            "legacy Office file changed while opening; retry the read"
        );
        let (command, path) = office_read::legacy_conversion(&input, target, directory.path())?;
        Ok::<_, anyhow::Error>((directory, input, command, path))
    })
    .await
    .context("join legacy Office conversion preparation")??;
    let output = checked(
        capture(command, ".txt")
            .await
            .context("convert legacy Office file with LibreOffice (soffice)")?,
    )?;
    ensure!(
        path.is_file(),
        "LibreOffice did not export {target}: {}",
        preview_text(&output.stderr)?
    );
    ensure!(
        path.metadata()?.len() <= office_read::MAX_FILE_BYTES,
        "converted Office file exceeds 64 MiB"
    );
    let _ = std::fs::remove_file(input);
    Ok(Arc::new(Converted {
        _directory: directory,
        path,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_tracks_contents_and_format_and_bounds_retention() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let cache = Cache::default();
        std::fs::write(file.path(), b"version a").unwrap();
        let key = Key {
            format: Format::Xls,
            digest: snapshot(file.path(), io::sink()).unwrap(),
        };
        let first = cache.entry(key).await;
        assert!(Arc::ptr_eq(&first, &cache.entry(key).await));
        let other_format = Key {
            format: Format::Ppt,
            ..key
        };
        assert!(!Arc::ptr_eq(&first, &cache.entry(other_format).await));
        std::fs::write(file.path(), b"version b").unwrap();
        let edited = Key {
            digest: snapshot(file.path(), io::sink()).unwrap(),
            ..key
        };
        assert!(!Arc::ptr_eq(&first, &cache.entry(edited).await));
        for digest in [1, 2, 3, 4] {
            let completed = cache
                .entry(Key {
                    digest: [digest; 32],
                    ..key
                })
                .await;
            completed
                .set(Arc::new(Converted {
                    _directory: tempfile::tempdir().unwrap(),
                    path: PathBuf::new(),
                }))
                .unwrap_or_else(|_| panic!("fresh cache entry"));
        }
        cache.prune().await;
        assert!(Arc::ptr_eq(&first, &cache.entry(key).await));
        assert_eq!(cache.entries.lock().await.len(), MAX_ENTRIES + 1);
        drop(first);
        cache.prune().await;
        assert_eq!(cache.entries.lock().await.len(), MAX_ENTRIES);
    }
}
