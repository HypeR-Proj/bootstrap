// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Reserve board-authored guest slots without admitting system clients.
//! The formal I/O runtime starts admission only after adopting this inventory.

use super::{Result, show};
use crate::contracts::io;
use hyper_io_runtime::guest::install::InstalledGuest;
use hyper_io_runtime::image::guest_fdt::io::{DmaRange, IoClient, MmioDevice, SharedMemory};
use hyper_os::guest_io::Mailbox;
use hyper_os::handle::{CapabilityChannelObject, OwnedHandle};
use hyper_os::vm;
use std::io::Read;

pub(super) struct Broker {
    pub endpoint: OwnedHandle<CapabilityChannelObject>,
    pub slots: Vec<ClientSlot>,
}
pub(super) struct ClientSlot {
    pub policy: hyper_io_runtime::clients::Client,
    pub mailbox: Option<Mailbox>,
}
impl Broker {
    pub(super) fn load(
        endpoint: OwnedHandle<CapabilityChannelObject>,
        environment: &hyper_os::fs::Directory,
        storage: &hyper_io_runtime::storage_policy::Policy,
    ) -> Result<Self> {
        let mut bytes = String::new();
        environment
            .open("etc/hyper/io-clients.conf", hyper_os::fs::FileRights::READ)
            .map_err(show)?
            .into_std()
            .take(8193)
            .read_to_string(&mut bytes)
            .map_err(show)?;
        let clients = hyper_io_runtime::clients::parse(&bytes).map_err(show)?;
        if clients.len() + 1 > vm::IO_MAX_CLIENTS
            || clients
                .iter()
                .any(|client| client.id as usize >= vm::IO_MAX_CLIENTS)
        {
            return Err("too many configured I/O clients".into());
        }
        storage.validate_broker(&clients)?;
        Ok(Self {
            endpoint,
            slots: clients
                .into_iter()
                .map(|policy| ClientSlot {
                    policy,
                    mailbox: None,
                })
                .collect(),
        })
    }
    pub(super) fn descriptions(&self) -> Vec<IoClient> {
        self.slots
            .iter()
            .map(|slot| {
                let index = slot.policy.id;
                IoClient {
                    id: slot.policy.id,
                    dynamic: true,
                    shared_memory: SharedMemory {
                        base: vm::DYNAMIC_ALIAS_OFFSET,
                        size: vm::DYNAMIC_PHYSICAL_LIMIT,
                        guest_base: 0,
                    },
                    mailbox: MmioDevice {
                        base: 0x0a01_0000 + u64::from(index) * 0x20000,
                        size: 4096,
                        irq: 41 + index * 2,
                    },
                    notification: slot.policy.volume.as_ref().map(|_| MmioDevice {
                        base: 0x0a02_0000 + u64::from(index) * 0x20000,
                        size: 4096,
                        irq: 42 + index * 2,
                    }),
                    network_notification: slot.policy.network.as_ref().map(|_| MmioDevice {
                        base: io::NETWORK_NOTIFICATION_BASE
                            + u64::from(index) * io::NETWORK_NOTIFICATION_STRIDE,
                        size: 4096,
                        irq: io::NETWORK_NOTIFICATION_IRQ_BASE + index,
                    }),
                }
            })
            .collect()
    }
    pub(super) fn dma_ranges(&self, excluded: &[DmaRange]) -> Result<Vec<DmaRange>> {
        // No dynamic client means no guest-memory aperture to translate.
        if self.slots.is_empty() {
            return Ok(Vec::new());
        }
        hyper_io_runtime::clients::dynamic_dma_ranges(excluded)
    }
    pub(super) fn install(&mut self, guest: &InstalledGuest) -> Result<()> {
        let descriptions = self.descriptions();
        for (slot, description) in self.slots.iter_mut().zip(descriptions) {
            slot.mailbox = Some(
                Mailbox::create(
                    guest.machine.as_handle_ref(),
                    description.mailbox.base,
                    description.mailbox.irq,
                )
                .map_err(show)?,
            );
        }
        Ok(())
    }
}
