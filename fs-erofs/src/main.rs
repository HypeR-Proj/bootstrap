// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Read-only EROFS worker; common filesystem service code owns IPC and mounts.

#[path = "../../logging.rs"]
mod logging;

#[path = "../../contracts/mod.rs"]
mod contracts;
#[path = "../../fs-backend/src/worker_transport.rs"]
mod worker_transport;

use hyper_filesystem::{
    Timestamp,
    protocol::{self, Entry, Error},
};
use hyper_fs_erofs::{Device as ReadDevice, Error as MediaError, Kind};
use hyper_os::block::NativeBlock;
use worker_transport::Volume;

struct Device {
    block: NativeBlock,
    bytes: u64,
}

impl ReadDevice for Device {
    fn len(&self) -> u64 {
        self.bytes
    }

    fn read_exact_at(&mut self, offset: u64, mut output: &mut [u8]) -> Result<(), MediaError> {
        if offset
            .checked_add(output.len() as u64)
            .is_none_or(|end| end > self.bytes)
        {
            return Err(MediaError::Corrupt);
        }
        let mut position = offset;
        let mut sector = [0; 512];
        while !output.is_empty() {
            let within = position as usize % sector.len();
            let count = if within != 0 || output.len() < sector.len() {
                self.block
                    .read_sectors(position / 512, &mut sector)
                    .map_err(|_| MediaError::Io)?;
                let count = output.len().min(sector.len() - within);
                output[..count].copy_from_slice(&sector[within..within + count]);
                count
            } else {
                let count = output.len().min(protocol::DATA_BYTES) / 512 * 512;
                self.block
                    .read_sectors(position / 512, &mut output[..count])
                    .map_err(|_| MediaError::Io)?;
                count
            };
            position += count as u64;
            output = &mut output[count..];
        }
        Ok(())
    }
}

struct Erofs(hyper_fs_erofs::Volume<Device>);

impl Volume for Erofs {
    fn stat(&mut self, path: &str) -> Result<Entry, Error> {
        metadata(self.0.stat(path).map_err(map_error)?)
    }
    fn read_directory(
        &mut self,
        path: &str,
        cursor: protocol::DirectoryCursor,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        self.0
            .read_directory(path, cursor, output)
            .map_err(map_error)
    }

    fn read(&mut self, path: &str, offset: u64, output: &mut [u8]) -> Result<usize, Error> {
        self.0.read(path, offset, output).map_err(map_error)
    }
    fn readlink(&mut self, path: &str, output: &mut [u8]) -> Result<usize, Error> {
        self.0.readlink(path, output).map_err(map_error)
    }
    fn write(&mut self, _: &str, _: u64, _: &[u8]) -> Result<usize, Error> {
        Err(Error::ReadOnly)
    }
    fn create(&mut self, _: &str, _: bool) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
    fn remove(&mut self, _: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
    fn rename(&mut self, _: &str, _: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
    fn resize(&mut self, _: &str, _: u64) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
    fn sync(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn set_times(
        &mut self,
        _: &str,
        _: Option<Timestamp>,
        _: Option<Timestamp>,
    ) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
}

fn metadata(value: hyper_fs_erofs::Entry) -> Result<Entry, Error> {
    let mut result = Entry::empty();
    result
        .name
        .get_mut(..value.name.len())
        .ok_or(Error::Corrupt)?
        .copy_from_slice(value.name.as_bytes());
    result.name_len = value.name.len();
    result.kind = match value.kind {
        Kind::File => protocol::FILE,
        Kind::Directory => protocol::DIRECTORY,
        Kind::Symlink => protocol::SYMLINK,
    };
    result.mode = value.mode & !0o222;
    result.size = value.size;
    result.modified = Timestamp::new(value.modified.0, value.modified.1);
    Ok(result)
}

fn map_error(error: MediaError) -> Error {
    match error {
        MediaError::Io => Error::Io,
        MediaError::Corrupt => Error::Corrupt,
        MediaError::Unsupported => Error::Unsupported,
        MediaError::Missing => Error::Missing,
        MediaError::InvalidInput => Error::InvalidInput,
        MediaError::Allocation => Error::Allocation,
        MediaError::ResourceLimit => Error::ResourceLimit,
    }
}

fn run() -> Result<(), String> {
    let mut startup = hyper_rt::process::startup().map_err(show)?;
    let sectors = std::env::args()
        .nth(1)
        .ok_or("missing volume geometry")?
        .parse::<u64>()
        .map_err(show)?;
    if std::env::args().nth(2).as_deref() != Some("ro") {
        return Err("EROFS requires a read-only mount".into());
    }
    let block = NativeBlock::from_handle(
        startup
            .take(crate::contracts::filesystem::BLOCK)
            .map_err(show)?,
    );
    let bytes = sectors.checked_mul(512).ok_or("invalid volume geometry")?;
    let mut volume = Erofs(hyper_fs_erofs::Volume::mount(Device { block, bytes }).map_err(show)?);
    let completion = worker_transport::serve(&mut startup, &mut volume)?;
    worker_transport::return_block(completion, volume.0.into_device().block)
}

fn show(error: impl core::fmt::Debug) -> String {
    format!("{error:?}")
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}{error}", crate::logging::prefix("fs-erofs"));
            std::process::ExitCode::FAILURE
        }
    }
}
