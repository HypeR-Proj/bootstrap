// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use crate::{
    Device, Error, Kind,
    reader::{Reader, u16_at, u32_at, u64_at},
    superblock::Superblock,
};

#[derive(Clone, Copy)]
pub(crate) struct Inode {
    pub nid: u64,
    pub kind: Kind,
    pub mode: u32,
    pub size: u64,
    pub modified: (i64, u32),
    data: u64,
    inline: Option<u64>,
}

impl Inode {
    pub fn load<D: Device>(
        reader: &mut Reader<D>,
        sb: &Superblock,
        nid: u64,
    ) -> Result<Self, Error> {
        let offset = nid
            .checked_mul(32)
            .and_then(|value| sb.metadata.checked_add(value))
            .ok_or(Error::Corrupt)?;
        let mut bytes = [0; 64];
        reader.metadata(offset, &mut bytes[..32])?;
        let format = u16_at(&bytes, 0)?;
        // Bit 4 and compact bytes 12..16 have meaning only with the unsupported
        // 48-bit incompatible feature; the core format explicitly ignores them.
        if format & !0x1f != 0 || !matches!((format >> 1) & 7, 0 | 2) || u16_at(&bytes, 2)? != 0 {
            return Err(Error::Unsupported);
        }
        let extended = format & 1 != 0;
        let inode_size = if extended { 64 } else { 32 };
        if extended {
            reader.metadata(offset + 32, &mut bytes[32..])?;
            if u16_at(&bytes, 6)? != 0 || bytes[48..64].iter().any(|byte| *byte != 0) {
                return Err(Error::Corrupt);
            }
        } else if u32_at(&bytes, 28)? != 0 {
            return Err(Error::Corrupt);
        }
        let mode = u16_at(&bytes, 4)?;
        let kind = match mode & 0xf000 {
            0x8000 => Kind::File,
            0x4000 => Kind::Directory,
            0xa000 => Kind::Symlink,
            _ => return Err(Error::Unsupported),
        };
        let size = if extended {
            u64_at(&bytes, 8)?
        } else {
            u64::from(u32_at(&bytes, 8)?)
        };
        let modified = if extended {
            (
                i64::try_from(u64_at(&bytes, 32)?).map_err(|_| Error::Corrupt)?,
                u32_at(&bytes, 40)?,
            )
        } else {
            sb.modified
        };
        if modified.1 >= 1_000_000_000 {
            return Err(Error::Corrupt);
        }
        let data = u64::from(u32_at(&bytes, 16)?) * sb.block_size as u64;
        let tail = size % sb.block_size as u64;
        let inline = if (format >> 1) & 7 == 2 {
            let start = offset.checked_add(inode_size).ok_or(Error::Corrupt)?;
            if tail == 0 || start % sb.block_size as u64 + tail > sb.block_size as u64 {
                return Err(Error::Corrupt);
            }
            reader.check(start, tail as usize)?;
            Some(start)
        } else {
            None
        };
        let external = if inline.is_some() { size - tail } else { size };
        if external != 0 {
            reader.check(data, usize::try_from(external).map_err(|_| Error::Corrupt)?)?;
        }
        Ok(Self {
            nid,
            kind,
            mode: u32::from(mode & 0o777),
            size,
            modified,
            data,
            inline,
        })
    }

    pub fn read<D: Device>(
        self,
        reader: &mut Reader<D>,
        sb: &Superblock,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        if offset >= self.size || output.is_empty() {
            return Ok(0);
        }
        let length = core::cmp::min(self.size - offset, output.len() as u64) as usize;
        let output = &mut output[..length];
        let boundary = if self.inline.is_some() {
            self.size / sb.block_size as u64 * sb.block_size as u64
        } else {
            self.size
        };
        let external = if offset < boundary {
            core::cmp::min(boundary - offset, length as u64) as usize
        } else {
            0
        };
        if external != 0 {
            reader.read(
                self.data.checked_add(offset).ok_or(Error::Corrupt)?,
                &mut output[..external],
            )?;
        }
        if external < length {
            let position = offset + external as u64 - boundary;
            let start = self
                .inline
                .ok_or(Error::Corrupt)?
                .checked_add(position)
                .ok_or(Error::Corrupt)?;
            reader.metadata(start, &mut output[external..])?;
        }
        Ok(length)
    }
}
