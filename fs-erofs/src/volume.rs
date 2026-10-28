// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use alloc::string::String;

use crate::{Device, Error, directory, inode::Inode, reader::Reader, superblock::Superblock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub mode: u32,
    pub size: u64,
    pub modified: (i64, u32),
}

/// Immutable media semantics with a bounded metadata block and last-path cache.
/// File data is read directly into the caller's range; whole images and files
/// are never buffered by this parser.
pub struct Volume<D> {
    reader: Reader<D>,
    superblock: Superblock,
    root: Inode,
    last: Option<(String, Inode)>,
}

impl<D: Device> Volume<D> {
    /// Returns a bounded batch using a trusted kernel-owned checkpoint. The
    /// immutable inode number avoids repeating parent-path searches on resume.
    pub fn read_directory(
        &mut self,
        path: &str,
        cursor: hyper_filesystem::protocol::DirectoryCursor,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        use hyper_filesystem::protocol::DirectoryBatchBuilder;
        if !hyper_filesystem::protocol::valid_path(path) {
            return Err(Error::InvalidInput);
        }
        let mut batch = DirectoryBatchBuilder::new(output, cursor).ok_or(Error::InvalidInput)?;
        let path_key = path.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
        });
        let (inode, offset, index) = if cursor.is_start() {
            (self.lookup(path)?, 0, 0)
        } else {
            if cursor.words[0] != directory::CURSOR_TAG || cursor.words[3] != path_key {
                return Err(Error::InvalidInput);
            }
            (
                Inode::load(&mut self.reader, &self.superblock, cursor.words[1])?,
                u64::from(cursor.words[2] as u32),
                (cursor.words[2] >> 32) as usize,
            )
        };
        let eof = directory::read_batch(
            &mut self.reader,
            &self.superblock,
            inode,
            offset,
            index,
            path_key,
            &mut batch,
        )?;
        batch.finish(eof).ok_or(Error::Corrupt)
    }

    /// Return the original device after the service has stopped using the mount.
    pub fn into_device(self) -> D {
        self.reader.device
    }

    pub fn mount(device: D) -> Result<Self, Error> {
        let mut reader = Reader::new(device);
        let superblock = Superblock::load(&mut reader)?;
        let root = Inode::load(&mut reader, &superblock, superblock.root)?;
        if root.kind != Kind::Directory {
            return Err(Error::Corrupt);
        }
        // Validate the root's first directory block before exposing the mount.
        directory::find(&mut reader, &superblock, root, None)?;
        Ok(Self {
            reader,
            superblock,
            root,
            last: None,
        })
    }

    /// Resolve canonical relative paths without following symbolic links.
    /// Namespace-level symlink traversal belongs to the VFS caller.
    fn lookup(&mut self, path: &str) -> Result<Inode, Error> {
        if path.len() > 4096 || path.as_bytes().contains(&0) {
            return Err(Error::InvalidInput);
        }
        if path.is_empty() {
            return Ok(self.root);
        }
        if let Some((cached, inode)) = &self.last
            && cached == path
        {
            return Ok(*inode);
        }
        let mut inode = self.root;
        for part in path.split('/') {
            if part.is_empty() || part == "." || part == ".." || part.len() > 255 {
                return Err(Error::InvalidInput);
            }
            let child = directory::find(
                &mut self.reader,
                &self.superblock,
                inode,
                Some(part.as_bytes()),
            )?
            .ok_or(Error::Missing)?;
            inode = Inode::load(&mut self.reader, &self.superblock, child.nid)?;
            if child.kind.is_some_and(|kind| kind != inode.kind) {
                return Err(Error::Corrupt);
            }
        }
        self.last = Some((copy_string(path)?, inode));
        Ok(inode)
    }

    pub fn stat(&mut self, path: &str) -> Result<Entry, Error> {
        let inode = self.lookup(path)?;
        entry(inode, path.rsplit('/').next().ok_or(Error::InvalidInput)?)
    }

    pub fn read(&mut self, path: &str, offset: u64, output: &mut [u8]) -> Result<usize, Error> {
        let inode = self.lookup(path)?;
        if inode.kind != Kind::File {
            return Err(Error::InvalidInput);
        }
        inode.read(&mut self.reader, &self.superblock, offset, output)
    }

    /// Describe core-format file bytes, including physical inline tails. The
    /// kernel reads these ranges into private request/cache pages; the worker
    /// never receives writable mappings of those destinations.
    pub fn read_extents(
        &mut self,
        path: &str,
        offset: u64,
        length: usize,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        use hyper_filesystem::protocol::{DATA_BYTES, ReadExtentPlanBuilder};
        if length > DATA_BYTES {
            return Err(Error::InvalidInput);
        }
        let inode = self.lookup(path)?;
        if inode.kind != Kind::File {
            return Err(Error::InvalidInput);
        }
        let mut plan = ReadExtentPlanBuilder::new(output).ok_or(Error::InvalidInput)?;
        inode.read_extents(&self.superblock, offset, length, &mut plan)?;
        Ok(plan.finish())
    }

    /// Return the stored link bytes verbatim; do not join or normalize a target.
    pub fn readlink(&mut self, path: &str, output: &mut [u8]) -> Result<usize, Error> {
        let inode = self.lookup(path)?;
        if inode.kind != Kind::Symlink {
            return Err(Error::InvalidInput);
        }
        if inode.size == 0 || inode.size > 4096 {
            return Err(Error::Corrupt);
        }
        if inode.size > output.len() as u64 {
            return Err(Error::InvalidInput);
        }
        let count = inode.read(&mut self.reader, &self.superblock, 0, output)?;
        if output[..count].contains(&0) {
            return Err(Error::Corrupt);
        }
        Ok(count)
    }
}

fn entry(inode: Inode, name: &str) -> Result<Entry, Error> {
    Ok(Entry {
        name: copy_string(name)?,
        kind: inode.kind,
        mode: inode.mode,
        size: inode.size,
        modified: inode.modified,
    })
}

fn copy_string(value: &str) -> Result<String, Error> {
    let mut result = String::new();
    result
        .try_reserve_exact(value.len())
        .map_err(|_| Error::Allocation)?;
    result.push_str(value);
    Ok(result)
}
