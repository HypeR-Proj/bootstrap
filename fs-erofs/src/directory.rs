// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use crate::{
    Device, Error, Kind,
    inode::Inode,
    reader::{MAX_BLOCK, Reader, u16_at, u64_at},
    superblock::Superblock,
};

const DIRENT_BYTES: usize = 12;
const MAX_DIRECTORY_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const CURSOR_TAG: u64 = 0x4552_4f46_5344_0001;

/// Reads at most one wire batch without rescanning preceding directory blocks.
/// A resumed block is validated in full and against its predecessor, retaining
/// the same sorted-name checks as a cold scan without storing a whole index.
pub(crate) fn read_batch<D: Device>(
    reader: &mut Reader<D>,
    sb: &Superblock,
    inode: Inode,
    mut offset: u64,
    mut index: usize,
    path_key: u64,
    batch: &mut hyper_filesystem::protocol::DirectoryBatchBuilder<'_>,
) -> Result<bool, Error> {
    use hyper_filesystem::{
        Timestamp,
        protocol::{self, DirectoryCursor, Entry},
    };
    if inode.kind != Kind::Directory
        || offset > inode.size
        || !offset.is_multiple_of(sb.block_size as u64)
    {
        return Err(Error::InvalidInput);
    }
    if inode.size == 0 {
        return Err(Error::Corrupt);
    }
    if inode.size > MAX_DIRECTORY_BYTES {
        return Err(Error::ResourceLimit);
    }
    let mut buffer = [0; MAX_BLOCK];
    let mut previous = [0; 255];
    let mut previous_len = 0;
    if offset != 0 {
        let length = inode.read(
            reader,
            sb,
            offset - sb.block_size as u64,
            &mut buffer[..sb.block_size],
        )?;
        let block = &buffer[..length];
        let count = validate(block)?;
        let name = child(block, count - 1, count)?.0;
        previous[..name.len()].copy_from_slice(name);
        previous_len = name.len();
    }
    let mut entry = Entry::empty();
    while offset < inode.size {
        let length = inode.read(reader, sb, offset, &mut buffer[..sb.block_size])?;
        let block = &buffer[..length];
        let count = validate(block)?;
        if index > count {
            return Err(Error::InvalidInput);
        }
        if previous_len != 0 && child(block, 0, count)?.0 <= &previous[..previous_len] {
            return Err(Error::Corrupt);
        }
        while index < count {
            let (name, nid, kind) = child(block, index, count)?;
            index += 1;
            if matches!(name, b"." | b"..") {
                continue;
            }
            let metadata = Inode::load(reader, sb, nid)?;
            if kind.is_some_and(|kind| kind != metadata.kind) {
                return Err(Error::Corrupt);
            }
            core::str::from_utf8(name).map_err(|_| Error::Unsupported)?;
            entry.name[..name.len()].copy_from_slice(name);
            entry.name_len = name.len();
            entry.kind = match metadata.kind {
                Kind::File => protocol::FILE,
                Kind::Directory => protocol::DIRECTORY,
                Kind::Symlink => protocol::SYMLINK,
            };
            entry.mode = metadata.mode & !0o222;
            entry.size = metadata.size;
            entry.modified = Timestamp::new(metadata.modified.0, metadata.modified.1);
            // Keep the final block's end as an index within that block. Its
            // inline tail need not end on a filesystem-block boundary.
            let next = DirectoryCursor {
                words: [
                    CURSOR_TAG,
                    inode.nid,
                    offset | ((index as u64) << 32),
                    path_key,
                ],
            };
            if !batch.push(&entry, next).ok_or(Error::Corrupt)? {
                return Ok(false);
            }
        }
        let last = child(block, count - 1, count)?.0;
        previous[..last.len()].copy_from_slice(last);
        previous_len = last.len();
        offset += length as u64;
        index = 0;
    }
    Ok(true)
}

pub(crate) struct Child {
    pub nid: u64,
    pub kind: Option<Kind>,
}

/// Scans bounded directory blocks, validating the full block before returning
/// any entry. Names are sorted, unique, and cannot escape a path component.
pub(crate) fn find<D: Device>(
    reader: &mut Reader<D>,
    sb: &Superblock,
    inode: Inode,
    name: Option<&[u8]>,
) -> Result<Option<Child>, Error> {
    if inode.kind != Kind::Directory {
        return Err(Error::InvalidInput);
    }
    if inode.size == 0 {
        return Err(Error::Corrupt);
    }
    if inode.size > MAX_DIRECTORY_BYTES {
        return Err(Error::ResourceLimit);
    }
    let mut buffer = [0; MAX_BLOCK];
    let mut offset = 0;
    let mut previous_block_last = [0; 255];
    let mut previous_length = 0;
    while offset < inode.size {
        let length = inode.read(reader, sb, offset, &mut buffer[..sb.block_size])?;
        let block = &buffer[..length];
        let count = validate(block)?;
        let first = child(block, 0, count)?;
        if previous_length != 0 && first.0 <= &previous_block_last[..previous_length] {
            return Err(Error::Corrupt);
        }
        let last = child(block, count - 1, count)?.0;
        previous_block_last[..last.len()].copy_from_slice(last);
        previous_length = last.len();
        for position in 0..count {
            let (entry_name, nid, kind) = child(block, position, count)?;
            if matches!(entry_name, b"." | b"..") {
                continue;
            }
            if name.is_none_or(|name| name == entry_name) {
                return Ok(Some(Child { nid, kind }));
            }
        }
        offset += length as u64;
    }
    Ok(None)
}

fn validate(block: &[u8]) -> Result<usize, Error> {
    let table = usize::from(u16_at(block, 8)?);
    if table == 0 || table >= block.len() || table % DIRENT_BYTES != 0 {
        return Err(Error::Corrupt);
    }
    let count = table / DIRENT_BYTES;
    let mut previous: &[u8] = &[];
    for index in 0..count {
        let (name, _, _) = child(block, index, count)?;
        if name <= previous {
            return Err(Error::Corrupt);
        }
        previous = name;
    }
    Ok(count)
}

fn child(block: &[u8], index: usize, count: usize) -> Result<(&[u8], u64, Option<Kind>), Error> {
    let record = index.checked_mul(DIRENT_BYTES).ok_or(Error::Corrupt)?;
    let start = usize::from(u16_at(block, record + 8)?);
    let end = if index + 1 == count {
        block.len()
    } else {
        usize::from(u16_at(block, record + DIRENT_BYTES + 8)?)
    };
    if start < count * DIRENT_BYTES || start >= end {
        return Err(Error::Corrupt);
    }
    let mut name = block.get(start..end).ok_or(Error::Corrupt)?;
    if index + 1 == count {
        let length = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len());
        // Only the first padding byte terminates the final name; the on-disk
        // format does not require subsequent unused bytes to be zero.
        name = &name[..length];
    }
    if name.is_empty()
        || name.len() > 255
        || name.contains(&0)
        || name.contains(&b'/')
        || block.get(record + 11) != Some(&0)
    {
        return Err(Error::Corrupt);
    }
    let kind = match block.get(record + 10).copied().ok_or(Error::Corrupt)? {
        0 => None,
        1 => Some(Kind::File),
        2 => Some(Kind::Directory),
        7 => Some(Kind::Symlink),
        3..=6 => return Err(Error::Unsupported),
        _ => return Err(Error::Corrupt),
    };
    Ok((name, u64_at(block, record)?, kind))
}
