// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use crate::{
    Device, Error,
    reader::{MAX_BLOCK, Reader, u16_at, u32_at, u64_at},
};

pub(crate) struct Superblock {
    pub block_size: usize,
    pub metadata: u64,
    pub root: u64,
    pub modified: (i64, u32),
}

impl Superblock {
    pub fn load<D: Device>(reader: &mut Reader<D>) -> Result<Self, Error> {
        let mut bytes = [0; 128];
        reader.read(1024, &mut bytes)?;
        if u32_at(&bytes, 0)? != 0xe0f5_e1e2 {
            return Err(Error::Corrupt);
        }
        let compat = u32_at(&bytes, 8)?;
        if !(9..=12).contains(&bytes[12])
            || bytes[13] != 0
            || compat & !3 != 0
            || u32_at(&bytes, 80)? != 0
            || u16_at(&bytes, 84)? != 0
            || bytes[90] != 0
        {
            return Err(Error::Unsupported);
        }
        let block_size = 1usize << bytes[12];
        if bytes[104] != 0
            || bytes[106..108].iter().any(|byte| *byte != 0)
            || bytes[120..128].iter().any(|byte| *byte != 0)
        {
            return Err(Error::Corrupt);
        }
        if compat & 1 != 0 {
            // The checksum covers the superblock through the end of its
            // filesystem block, not an additional block after offset 1024.
            let length = block_size - 1024 % block_size;
            let mut block = [0; MAX_BLOCK];
            reader.read(1024, &mut block[..length])?;
            let expected = u32_at(&bytes, 4)?;
            block[4..8].fill(0);
            if crc32c(&block[..length]) != expected {
                return Err(Error::Corrupt);
            }
        }
        let modified = (
            i64::try_from(u64_at(&bytes, 24)?).map_err(|_| Error::Corrupt)?,
            u32_at(&bytes, 32)?,
        );
        if modified.1 >= 1_000_000_000 {
            return Err(Error::Corrupt);
        }
        // The format permits blocks/inos to be zero for generated images;
        // these reporting fields do not bound readable storage. Device.len()
        // is the authority boundary for every checked metadata/data range.
        let metadata = u64::from(u32_at(&bytes, 40)?) * block_size as u64;
        reader.check(metadata, 32)?;
        Ok(Self {
            block_size,
            metadata,
            root: u64::from(u16_at(&bytes, 14)?),
            modified,
        })
    }
}

fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}
