// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use crate::{Device, Error};

pub(crate) const MAX_BLOCK: usize = 4096;

/// One bounded metadata cache; file payload reads use the caller's buffer.
pub(crate) struct Reader<D> {
    pub device: D,
    cache: [u8; MAX_BLOCK],
    cached_offset: Option<u64>,
    cached_length: usize,
}

impl<D: Device> Reader<D> {
    pub fn new(device: D) -> Self {
        Self {
            device,
            cache: [0; MAX_BLOCK],
            cached_offset: None,
            cached_length: 0,
        }
    }

    pub fn check(&self, offset: u64, length: usize) -> Result<(), Error> {
        let end = offset.checked_add(length as u64).ok_or(Error::Corrupt)?;
        if end > self.device.len() {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    pub fn read(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
        self.check(offset, output.len())?;
        if output.is_empty() {
            return Ok(());
        }
        self.device.read_exact_at(offset, output)
    }

    pub fn metadata(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
        self.check(offset, output.len())?;
        let mut position = offset;
        for chunk in output.chunks_mut(MAX_BLOCK) {
            let base = position / MAX_BLOCK as u64 * MAX_BLOCK as u64;
            if self.cached_offset != Some(base) {
                self.cached_offset = None;
                self.cached_length =
                    core::cmp::min(self.device.len() - base, MAX_BLOCK as u64) as usize;
                self.device
                    .read_exact_at(base, &mut self.cache[..self.cached_length])?;
                self.cached_offset = Some(base);
            }
            let start = (position - base) as usize;
            let count = core::cmp::min(chunk.len(), self.cached_length - start);
            chunk[..count].copy_from_slice(&self.cache[start..start + count]);
            if count != chunk.len() {
                // Inode records may cross a cache boundary. This tail is at
                // most one small record and does not evict the useful prefix.
                self.read(position + count as u64, &mut chunk[count..])?;
            }
            position += chunk.len() as u64;
        }
        Ok(())
    }
}

pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(Error::Corrupt)?
            .try_into()
            .map_err(|_| Error::Corrupt)?,
    ))
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::Corrupt)?
            .try_into()
            .map_err(|_| Error::Corrupt)?,
    ))
}

pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(Error::Corrupt)?
            .try_into()
            .map_err(|_| Error::Corrupt)?,
    ))
}
