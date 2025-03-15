use std::{
    any::Any,
    collections::HashMap,
    io::{Cursor, ErrorKind, Write},
    path::PathBuf,
};

use aaoffline::FileWriter;
use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use zip::{write::SimpleFileOptions, ZipWriter};

#[derive(Debug)]
pub(crate) struct WasmWriter {
    buffer: Mutex<HashMap<PathBuf, VirtualFsEntry>>,
}

#[derive(Debug, PartialEq, Eq)]
enum VirtualFsEntry {
    File(Vec<u8>),
    Symlink(PathBuf),
    Directory,
}

impl WasmWriter {
    pub(crate) fn new() -> WasmWriter {
        WasmWriter {
            buffer: Mutex::new(HashMap::new()),
        }
    }

    fn file_cursor<'a>(
        buf: &'a mut HashMap<PathBuf, VirtualFsEntry>,
        path: &std::path::Path,
    ) -> Cursor<&'a mut Vec<u8>> {
        if !buf.contains_key(path) {
            let content = Vec::new();
            buf.insert(path.to_path_buf(), VirtualFsEntry::File(content));
        }
        let content = buf.get_mut(path).unwrap();
        if let VirtualFsEntry::File(file) = content {
            Cursor::new(file)
        } else {
            panic!("queried file was actually {content:?}")
        }
    }

    pub(crate) fn finish(&self) -> Result<Vec<u8>> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        let buffer = self.buffer.lock();
        let (directories, files): (Vec<(_, _)>, Vec<(_, _)>) = buffer
            .iter()
            .partition(|x| matches!(x.1, VirtualFsEntry::Directory));
        for (path, _) in directories {
            writer.add_directory(path.to_str().expect("invalid path encountered"), options)?;
        }
        for (path, entry) in files {
            match entry {
                VirtualFsEntry::File(content) => {
                    writer.start_file_from_path(path, options)?;
                    writer.write_all(content)?;
                }
                VirtualFsEntry::Symlink(target) => {
                    writer.add_symlink_from_path(path, target, options)?;
                }
                _ => unreachable!(),
            }
        }
        writer.finish().map(|x| x.into_inner()).map_err(Into::into)
    }
}

// TODO: async-trait
#[async_trait]
impl FileWriter for WasmWriter {
    async fn write(&self, path: &std::path::Path, content: &[u8]) -> Result<(), std::io::Error> {
        let mut buf = self.buffer.lock();
        Self::file_cursor(&mut buf, path).write_all(content)
    }

    async fn symlink(
        &self,
        orig: &std::path::Path,
        target: &std::path::Path,
    ) -> Result<(), std::io::Error> {
        let mut buf = self.buffer.lock();
        if let Some(existing) = buf.insert(
            target.to_path_buf(),
            VirtualFsEntry::Symlink(orig.to_path_buf()),
        ) {
            Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!("target should be empty when creating symlink: {existing:?}"),
            ))
        } else {
            Ok(())
        }
    }

    async fn hardlink(&self, _: &std::path::Path, _: &std::path::Path) {
        unimplemented!("cannot hard-link in ZIP files")
    }

    async fn delete_case_at(&self, _: &std::path::Path) {
        // No need to do anything here—the ZIP file will be empty at first.
    }

    async fn create_dir_all(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        let mut buf = self.buffer.lock();
        if let Some(existing) = buf
            .insert(path.to_path_buf(), VirtualFsEntry::Directory)
            .filter(|x| x != &VirtualFsEntry::Directory)
        {
            Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!("target should be empty when creating directory: {existing:?}"),
            ))
        } else {
            Ok(())
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
