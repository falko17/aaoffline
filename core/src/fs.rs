//! Structs and methods related to interaction with the filesystem.

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use futures::TryFutureExt;
use log::warn;
use tokio::io;

use crate::FileWriter;

/// A writer that uses the utilities provided by [`tokio::fs`] to asynchronously
/// interact with the local filesystem.
#[derive(Debug)]
pub struct TokioFsWriter;

#[async_trait]
impl FileWriter for TokioFsWriter {
    async fn write(&self, path: &Path, content: &[u8]) -> Result<(), std::io::Error> {
        tokio::fs::write(path, content).await
    }

    #[cfg(unix)]
    async fn symlink(
        &self,
        orig: &std::path::Path,
        target: &std::path::Path,
    ) -> Result<(), std::io::Error> {
        if let Err(e) = tokio::fs::remove_file(target).await
            && e.kind() != io::ErrorKind::NotFound
        {
            return Err(e);
        }
        tokio::fs::symlink(orig, target).await
    }

    #[cfg(windows)]
    async fn symlink(
        &self,
        orig: &std::path::Path,
        target: &std::path::Path,
    ) -> Result<(), std::io::Error> {
        if let Err(e) = tokio::fs::remove_file(target).await
            && e.kind() != io::ErrorKind::NotFound
        {
            return Err(e);
        }
        tokio::fs::symlink_file(orig, target).await
    }

    async fn hardlink(&self, orig: &std::path::Path, target: &std::path::Path) {
        // Hard-links will only work if there's already a file here, so we'll
        // create a place-holder.
        tokio::fs::File::create(orig)
            .await
            .expect("Could not create placeholder for hard link");
        tokio::fs::hard_link(orig, target)
            .await
            .expect("Could not hard-link file");
    }

    async fn delete_case_at(&self, output: &std::path::Path) {
        if Path::new(output).is_file() {
            // We will simply remove the file if the output is a file.
            tokio::fs::remove_file(output).await.unwrap_or_else(|e| {
                if let io::ErrorKind::NotFound = e.kind() {
                    // Ignore if already deleted.
                } else {
                    warn!(
                        "Could not remove file {}: {e}. Please remove it manually.",
                        output.display()
                    );
                }
            });
        } else if Path::new(output).is_dir() {
            // We need to remove the assets folder and the index.html file.
            tokio::fs::remove_dir_all(output.join("assets"))
                .and_then(|()| tokio::fs::remove_file(output.join("index.html")))
                .await
                .unwrap_or_else(|e| {
                    if let io::ErrorKind::NotFound = e.kind() {
                        // Ignore if already deleted.
                    } else {
                        warn!(
                            "Could not remove content in {}: {e}. Please remove manually.",
                            output.display()
                        );
                    }
                });
        }
    }

    async fn create_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        tokio::fs::create_dir_all(path).await
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
