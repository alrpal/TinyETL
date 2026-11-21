//! Output files that appear only when complete.
//!
//! File targets write to a temporary file in the destination's directory and rename it over the
//! destination when the transfer finishes. A failed or interrupted run leaves any existing file
//! untouched, and the temporary file is removed when it's dropped.

use std::fs::File;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use crate::{Result, TinyEtlError};

pub(crate) struct OutputFile {
    temp: NamedTempFile,
    path: PathBuf,
}

impl OutputFile {
    /// Start writing `path`. Returns the handle to commit later and a file to write to.
    pub fn create(path: &Path) -> Result<(Self, File)> {
        let dir = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        std::fs::create_dir_all(&dir)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".to_string());
        let temp = tempfile::Builder::new()
            .prefix(&format!(".{}.", name))
            .suffix(".tmp")
            .tempfile_in(&dir)
            .map_err(|e| {
                TinyEtlError::Io(std::io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to create a temporary file in {}: {}",
                        dir.display(),
                        e
                    ),
                ))
            })?;
        let file = temp.reopen()?;
        Ok((
            Self {
                temp,
                path: path.to_path_buf(),
            },
            file,
        ))
    }

    /// Replace the destination with the finished file. Call after the writer is flushed.
    pub fn commit(self) -> Result<()> {
        let path = self.path;
        self.temp.persist(&path).map_err(|e| {
            TinyEtlError::Io(std::io::Error::new(
                e.error.kind(),
                format!("Failed to write {}: {}", path.display(), e.error),
            ))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_commit_replaces_and_drop_discards() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, "old").unwrap();

        // Dropped without commit: the old file stays and no temp file is left behind
        let (output, mut file) = OutputFile::create(&path).unwrap();
        file.write_all(b"partial").unwrap();
        drop(output);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        let (output, mut file) = OutputFile::create(&path).unwrap();
        file.write_all(b"new").unwrap();
        drop(file);
        output.commit().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn test_creates_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/out.txt");
        let (output, _file) = OutputFile::create(&path).unwrap();
        output.commit().unwrap();
        assert!(path.exists());
    }
}
