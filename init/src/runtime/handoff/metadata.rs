// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Immutable handoff description travels separately from live authority.

use super::{
    Runtime,
    inventory::{InventoryOffer, Slot},
};
use crate::manifest::Manifest;
use hyper_os::handle::{OwnedHandle, TaskGroupObject, VmoObject};
use hyper_os::memory::{PAGE_SIZE, SnapshotVmo, WritableVmo};
use hyper_os::startup::StartupPurpose;
use serde::Serialize;
use std::io;

pub(super) const SNAPSHOT: StartupPurpose<VmoObject> = StartupPurpose::new(0x8008_0002);
const MAX_OFFER_BYTES: usize = 32 * 1024;

#[derive(Serialize)]
pub(super) struct PreparedOffer<'a> {
    version: u32,
    root_mount: u64,
    boot_root: &'a str,
    services: Vec<CoreService<'a>>,
    pub(super) storage_ready: bool,
    pub(super) slots: Vec<Slot>,
}

#[derive(Serialize)]
struct CoreService<'a> {
    name: &'a str,
    image: &'a str,
    critical: bool,
    provider: Option<&'static str>,
}

impl<'a> PreparedOffer<'a> {
    pub(super) fn new(
        runtime: &Runtime,
        group: &OwnedHandle<TaskGroupObject>,
        core: &'a Manifest<'_>,
        boot_root: &'a str,
    ) -> hyper_os::Result<Self> {
        let mut inventory = InventoryOffer::new(&runtime.launcher.authorities, group)?;
        inventory.describe_transition(
            runtime
                .root_transition
                .as_ref()
                .ok_or(hyper_os::Error::MissingHandle)?,
        )?;
        inventory.describe_processes(&runtime.supervisors)?;
        let mut services = super::reserved_vec(core.service_count())?;
        for service in core.services() {
            services.push(CoreService {
                name: service.name(),
                image: service.image(),
                critical: service.critical(),
                provider: service.capabilities().find_map(|capability| {
                    match capability.purpose() {
                        "io.device-authority" => Some("io"),
                        "filesystem.attach" => Some("filesystem"),
                        "console.system" => Some("console"),
                        _ => None,
                    }
                }),
            });
        }
        Ok(Self {
            version: 3,
            root_mount: runtime.system_root_mount,
            boot_root,
            services,
            storage_ready: runtime.storage_ready,
            slots: inventory.slots,
        })
    }

    pub(super) fn snapshot(&self) -> hyper_os::Result<SnapshotVmo> {
        let bytes = encode(self)?;
        let size = (bytes.len() as u64 + 8).div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let vmo = WritableVmo::create(size)?;
        vmo.write_all_at(0, &(bytes.len() as u64).to_le_bytes())?;
        vmo.write_all_at(8, &bytes)?;
        SnapshotVmo::from_vmo(vmo.as_handle_ref())
    }
}

/// Count escaped JSON bytes before allocating. The second pass writes into a
/// fixed slice, so serde cannot grow an infallible Vec while a root is provisional.
fn encode(offer: &PreparedOffer<'_>) -> hyper_os::Result<Vec<u8>> {
    struct Length(usize);
    impl io::Write for Length {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len()).min(MAX_OFFER_BYTES + 1);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut length = Length(0);
    serde_json::to_writer(&mut length, offer).map_err(|_| hyper_os::Error::InvalidResponse)?;
    if length.0 > MAX_OFFER_BYTES {
        return Err(hyper_os::Error::InvalidResponse);
    }
    let mut bytes = super::reserved_vec(length.0)?;
    bytes.resize(length.0, 0);
    let mut output = io::Cursor::new(bytes.as_mut_slice());
    serde_json::to_writer(&mut output, offer).map_err(|_| hyper_os::Error::InvalidResponse)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_encoding_counts_json_escapes_and_preserves_wire_shape() {
        let offer = PreparedOffer {
            version: 3,
            root_mount: u64::MAX,
            boot_root: "/boot",
            services: vec![CoreService {
                name: "console",
                image: "/svc/quoted\"\\\nimage",
                critical: true,
                provider: Some("console"),
            }],
            storage_ready: true,
            slots: Vec::new(),
        };
        let bytes = encode(&offer);
        assert!(bytes.is_ok());
        assert_eq!(bytes.ok(), serde_json::to_vec(&offer).ok());
    }

    #[test]
    fn oversized_offer_and_impossible_reservation_return_errors() {
        let oversized = "x".repeat(MAX_OFFER_BYTES);
        let offer = PreparedOffer {
            version: 3,
            root_mount: 1,
            boot_root: &oversized,
            services: Vec::new(),
            storage_ready: false,
            slots: Vec::new(),
        };
        assert!(matches!(
            encode(&offer),
            Err(hyper_os::Error::InvalidResponse)
        ));
        assert!(matches!(
            super::super::reserved_vec::<u8>(usize::MAX),
            Err(hyper_os::Error::Status(hyper_os::Status::NO_MEMORY))
        ));
    }
}
