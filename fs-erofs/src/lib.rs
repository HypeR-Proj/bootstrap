// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bounded read-only EROFS core-format reader for an independent filesystem worker.
//!
//! Supports compact/extended inodes and uncompressed plain/inline file data.
//! Compression, chunks, xattrs and incompatible extensions are explicitly rejected.
//! Disk offsets are validated against the supplied device, never trusted metadata.
//! Format reference: <https://erofs.docs.kernel.org/en/latest/ondisk/core_ondisk.html>.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod directory;
mod inode;
mod reader;
mod superblock;
mod volume;

pub use volume::{Entry, Kind, Volume};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Io,
    Corrupt,
    Unsupported,
    Missing,
    InvalidInput,
    Allocation,
    ResourceLimit,
}

/// Exact random reads; implementations must reject ranges beyond `len()`.
/// No write interface is exposed to the media parser.
pub trait Device {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_exact_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<(), Error>;
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
#[path = "../tests/format.rs"]
mod tests;
