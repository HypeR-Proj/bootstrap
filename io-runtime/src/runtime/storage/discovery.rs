// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Discover actual media before authorizing immutable backend ranges.

use super::*;
use hyper_block::gpt;
use hyper_io_runtime::protocol::block::{self as block_protocol, Command, Disk, Reply};
use hyper_io_runtime::storage_policy::{
    Policy,
    catalog::{Catalog, Resolution},
};
use std::time::Duration;

struct Controller<'a> {
    scratch: &'a NativeStorage,
    mailbox: &'a Mailbox,
    guest: &'a mut InstalledGuest,
    log: &'a mut GuestLog,
    transaction: u64,
    max_read: usize,
    limit: u64,
}

impl Controller<'_> {
    fn exchange(&mut self, command: Command) -> Result<Reply> {
        // Completed replies win within an exchange, but a busy controller
        // cannot extend the discovery budget by always having another reply.
        check_deadline(self.limit)?;
        self.transaction = self
            .transaction
            .checked_add(1)
            .ok_or("block transaction exhausted")?;
        let request = block_protocol::Request {
            transaction: self.transaction,
            command,
        };
        let mut bytes = [0; MAX_RECORD];
        let length = request.encode(&mut bytes).map_err(show)?;
        self.mailbox.send(&bytes[..length]).map_err(show)?;
        let limit = self.limit;
        loop {
            if !pump_guest(self.guest, self.log)? {
                return Err("backend stopped during block management".into());
            }
            match self.mailbox.receive(&mut bytes) {
                Ok(length) => return request.decode_reply(&bytes[..length]).map_err(show),
                Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => {}
                Err(error) => return Err(show(error)),
            }
            check_deadline(limit)?;
            let items = [
                self.guest.output.wait_item(),
                WaitItem::new(
                    self.guest.machine.as_handle_ref(),
                    ObjectSignals::<VirtualMachineObject>::POWER_REQUEST
                        .union(ObjectSignals::<VirtualMachineObject>::VCPU_TERMINATED),
                ),
                WaitItem::new(
                    self.mailbox.as_handle_ref(),
                    ObjectSignals::<GuestMailboxObject>::READABLE
                        .union(ObjectSignals::<GuestMailboxObject>::PEER_CLOSED),
                ),
            ];
            let event = wait::wait_many(&items, limit).map_err(show)?;
            if event.index == 2
                && ObjectSignals::<GuestMailboxObject>::PEER_CLOSED.is_present_in(event.observed)
            {
                return Err("block management channel closed".into());
            }
        }
    }

    fn wait_probe(&mut self) -> Result<()> {
        let wake = hyper_os::time::deadline_after(Duration::from_millis(250))
            .map_err(show)?
            .as_raw()
            .min(self.limit);
        loop {
            if !pump_guest(self.guest, self.log)? {
                return Err("backend stopped during disk discovery".into());
            }
            if check_deadline(wake).is_err() {
                return Ok(());
            }
            let items = [
                self.guest.output.wait_item(),
                WaitItem::new(
                    self.guest.machine.as_handle_ref(),
                    ObjectSignals::<VirtualMachineObject>::POWER_REQUEST
                        .union(ObjectSignals::<VirtualMachineObject>::VCPU_TERMINATED),
                ),
            ];
            match wait::wait_many(&items, wake) {
                Ok(_) => {}
                Err(hyper_os::Error::Status(hyper_os::Status::TIMED_OUT)) => return Ok(()),
                Err(error) => return Err(show(error)),
            }
        }
    }

    fn read(&mut self, disk: Disk, offset: u64, bytes: &mut [u8]) -> Result<()> {
        let unit = disk.sector_bytes as usize;
        let chunk_size = self.max_read / unit * unit;
        if chunk_size == 0
            || !offset.is_multiple_of(unit as u64)
            || !bytes.len().is_multiple_of(unit)
            || offset
                .checked_add(bytes.len() as u64)
                .is_none_or(|end| end > disk.bytes)
        {
            return Err("invalid discovery read range".into());
        }
        for (index, chunk) in bytes.chunks_mut(chunk_size).enumerate() {
            self.exchange(Command::Read {
                disk: disk.id,
                offset: offset + (index * chunk_size) as u64,
                length: chunk.len() as u64,
                scratch: 0,
            })?;
            // The synchronous reply completes the backend write into this
            // private scratch grant; no filesystem or guest receives its handle.
            self.scratch
                .scratch
                .as_ref()
                .ok_or("missing local discovery mapping")?
                .read_exact_at(0, chunk)
                .map_err(show)?;
        }
        Ok(())
    }
}

pub(super) fn bind_partitions(
    clients: &mut [NativeStorage],
    mailboxes: &[Mailbox],
    policy: &Policy,
    guest: &mut InstalledGuest,
    log: &mut GuestLog,
    max_read: u32,
) -> Result<()> {
    if clients.len() != policy.native.len() + 1
        || clients
            .iter()
            .skip(1)
            .zip(&policy.native)
            .any(|(client, volume)| client.client != volume.client)
    {
        return Err("Native storage clients differ from partition policy".into());
    }
    let scratch = clients
        .first()
        .filter(|client| client.client == 0)
        .ok_or("missing storage controller")?;
    let mailbox = mailboxes.first().ok_or("missing storage control mailbox")?;
    scratch.prepare_memory(guest, mailbox, log)?;
    let mut controller = Controller {
        scratch,
        mailbox,
        guest,
        log,
        transaction: 2,
        max_read: (max_read as usize).min(block::MEMORY_BYTES as usize),
        limit: deadline(45)?,
    };
    let policies: Vec<_> = policy.native.iter().chain(&policy.guests).collect();
    let selected = loop {
        let catalog = discover(&mut controller)?;
        match catalog.resolve(policy)? {
            Resolution::Ready(selected) => break selected,
            Resolution::Missing(missing) => {
                if check_deadline(controller.limit).is_err() {
                    return Err(format!(
                        "partition discovery timed out: {}",
                        missing.join(", ")
                    ));
                }
                controller.wait_probe()?;
            }
        }
    };
    for (volume, selected) in policies.iter().zip(&selected) {
        controller.exchange(Command::Bind {
            client: volume.client,
            read_only: volume.read_only,
            disk: selected.disk.id,
            first: selected.partition.first_lba,
            sectors: selected.partition.sectors,
        })?;
    }
    for (client, selected) in clients.iter_mut().skip(1).zip(&selected) {
        client.partition = Some(selected.partition.clone());
    }
    Ok(())
}

fn discover(controller: &mut Controller<'_>) -> Result<Catalog> {
    let mut catalog = Catalog::default();
    let mut disks = Vec::new();
    let mut cursor = 0;
    loop {
        let Reply::Disk(disk) = controller.exchange(Command::Enumerate { cursor })? else {
            return Err("invalid disk enumeration response".into());
        };
        let Some(disk) = disk else { break };
        if disks.len() >= 128 || disks.contains(&disk.id) {
            return Err("duplicate or excessive backend disk enumeration".into());
        }
        disks.push(disk.id);
        cursor = disk.next_cursor;
        let table = gpt::read(
            disk.bytes / u64::from(disk.sector_bytes),
            disk.sector_bytes,
            |offset, bytes| controller.read(disk, offset, bytes),
        );
        match table {
            Ok(table) => {
                if table.degraded {
                    eprintln!(
                        "HypeR io-runtime: disk {} has only one valid GPT copy",
                        table.disk_id
                    );
                }
                catalog.extend(disk, table.partitions)?;
            }
            Err(gpt::Error::Invalid(reason)) => {
                eprintln!("HypeR io-runtime: disk {} skipped: {reason}", disk.id);
            }
            Err(error) => return Err(format!("GPT discovery: {error:?}")),
        }
    }
    Ok(catalog)
}
