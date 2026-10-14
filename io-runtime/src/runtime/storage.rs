// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Independent Native volume initiators with dedicated shared queue pools.
//! Userspace establishes them once; file I/O never traverses this supervisor.

use super::{Result, check_deadline, deadline, pump_guest, show};
use hyper_io_runtime::guest::install::{InstalledGuest, RAM_BASE, RAM_BYTES, SharedGrant};
use hyper_io_runtime::guest_log::GuestLog;
use hyper_io_runtime::image::guest_fdt::io::{DmaRange, IoClient, MmioDevice, SharedMemory};
use hyper_io_runtime::protocol::control::{Command, MAX_RECORD, Reply, Request, Status};
use hyper_io_runtime::protocol::{BackendOperation, QUEUES, Queue, VERSION_1};
use hyper_io_runtime::storage_policy::Volume;
use hyper_os::block::{self, NativeBlock};
use hyper_os::capability_channel::CapabilityChannel;
use hyper_os::device::{self, DmaExtent};
use hyper_os::guest_io::Mailbox;
use hyper_os::handle::{
    ByteChannelObject, DeviceAssignmentAuthorityObject, GuestMailboxObject, GuestMemoryObject,
    HandleRef, OwnedHandle, VirtualMachineObject, VmarObject,
};
use hyper_os::memory::{SharedReadMapping, WritableVmo};
use hyper_os::vm;
use hyper_os::wait::{self, ObjectSignals, WaitItem};

const _: () = assert!(QUEUES == block::QUEUE_COUNT);

const SHARED_BASE: u64 = RAM_BASE + RAM_BYTES;

mod discovery;
mod filesystem;

pub(super) struct NativeStorage {
    scratch: Option<SharedReadMapping>,
    // The guest-memory object pins the backing after the creation VMO closes.
    grant: OwnedHandle<GuestMemoryObject>,
    dma: DmaExtent,
    block: Option<NativeBlock>,
    client: u32,
    policy: Option<Volume>,
    partition: Option<hyper_io_runtime::partition::Partition>,
}
impl NativeStorage {
    pub(super) fn into_grant(self) -> Result<OwnedHandle<GuestMemoryObject>> {
        if self.block.is_some() {
            return Err("Native block initiator has not been published".into());
        }
        Ok(self.grant)
    }

    pub(super) fn prepare(
        authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
        root: HandleRef<'_, VmarObject>,
        client: u32,
        policy: Option<Volume>,
    ) -> Result<Self> {
        let memory = WritableVmo::create_contiguous(block::MEMORY_BYTES).map_err(show)?;
        let dma = device::dma_extent(authority, memory.as_handle_ref(), 0, block::MEMORY_BYTES)
            .map_err(show)?;
        let grant = vm::create_guest_memory(memory.as_handle_ref()).map_err(show)?;
        let scratch = if client == 0 {
            // SAFETY: root is this process's startup ROOT_VMAR. The private
            // child remains exclusively owned by NativeStorage until teardown.
            Some(unsafe { memory.map_shared_read(root) }.map_err(show)?)
        } else {
            None
        };
        Ok(Self {
            scratch,
            grant,
            dma,
            block: None,
            client,
            policy,
            partition: None,
        })
    }
    pub(super) fn description(&self) -> IoClient {
        IoClient {
            id: self.client,
            shared_memory: SharedMemory {
                base: self.shared_base(),
                size: block::MEMORY_BYTES,
                guest_base: self.shared_base(),
            },
            mailbox: MmioDevice {
                base: 0x0a01_0000 + u64::from(self.client) * 0x20000,
                size: 4096,
                irq: 41 + self.client * 2,
            },
            notification: (self.client != 0).then_some(MmioDevice {
                base: 0x0a02_0000 + u64::from(self.client) * 0x20000,
                size: 4096,
                irq: 42 + self.client * 2,
            }),
            network_notification: None,
            dynamic: false,
        }
    }
    pub(super) fn dma_range(&self) -> DmaRange {
        DmaRange {
            dma_base: self.dma.physical_base,
            cpu_base: self.shared_base(),
            size: block::MEMORY_BYTES,
        }
    }
    pub(super) fn mapping(&self) -> SharedGrant<'_> {
        SharedGrant {
            memory: self.grant.as_handle_ref(),
            guest_offset: self.shared_base() - RAM_BASE,
            memory_offset: 0,
            size: block::MEMORY_BYTES,
        }
    }
    fn shared_base(&self) -> u64 {
        SHARED_BASE + u64::from(self.client) * block::MEMORY_BYTES
    }
    pub(super) fn address_space_end(&self) -> u64 {
        self.shared_base() + block::MEMORY_BYTES - RAM_BASE
    }
    pub(super) fn install(&mut self, guest: &InstalledGuest) -> Result<()> {
        if self.client == 0 {
            return Ok(());
        }
        self.block = Some(
            NativeBlock::create(
                self.grant.as_handle_ref(),
                guest.machine.as_handle_ref(),
                self.shared_base(),
                0x0a02_0000 + u64::from(self.client) * 0x20000,
                42 + self.client * 2,
            )
            .map_err(show)?,
        );
        Ok(())
    }
    fn prepare_memory(
        &self,
        guest: &mut InstalledGuest,
        mailbox: &Mailbox,
        guest_log: &mut GuestLog,
    ) -> Result<()> {
        exchange(
            guest,
            guest_log,
            mailbox,
            Request {
                binding: 1,
                epoch: 1,
                transaction: 2,
                command: Command::Prepare {
                    alias: self.shared_base(),
                    guest_base: self.shared_base(),
                    length: block::MEMORY_BYTES,
                    mapping_token: 0,
                },
            },
        )?;
        Ok(())
    }
    fn configure(
        &mut self,
        guest: &mut InstalledGuest,
        mailbox: &Mailbox,
        guest_log: &mut GuestLog,
        features: u64,
    ) -> Result<()> {
        if features & VERSION_1 == 0 {
            return Err("backend lacks virtio version 1".into());
        }
        self.prepare_memory(guest, mailbox, guest_log)?;
        let queues: [Queue; QUEUES] = std::array::from_fn(|index| {
            let base = self.shared_base() + index as u64 * block::QUEUE_STRIDE;
            Queue {
                size: block::QUEUE_SIZE,
                descriptor: base,
                available: base + block::AVAILABLE_OFFSET,
                used: base + block::USED_OFFSET,
                ready: true,
            }
        });
        exchange(
            guest,
            guest_log,
            mailbox,
            Request {
                binding: 1,
                epoch: 1,
                transaction: 3,
                command: Command::Device(BackendOperation::Activate {
                    features: VERSION_1,
                    queues,
                }),
            },
        )?;
        Ok(())
    }
}

/// Attach all already-negotiated volumes before publishing bootstrap mount
/// readiness. Keep pumping the I/O VM while the filesystem manager starts its
/// independent workers; scope exit joins before any DMA grant can be dropped.
pub(super) fn mount_volumes(
    clients: &mut [NativeStorage],
    mailboxes: &[Mailbox],
    provider: CapabilityChannel,
    guest: &mut InstalledGuest,
    guest_log: &mut GuestLog,
    policy: &hyper_io_runtime::storage_policy::Policy,
    max_read: u32,
) -> Result<bool> {
    if clients.is_empty() || clients.len() != mailboxes.len() {
        return Err("Native volume and mailbox tables differ".into());
    }
    discovery::bind_partitions(clients, mailboxes, policy, guest, guest_log, max_read)?;
    let mut volumes = Vec::new();
    for (client, mailbox) in clients.iter_mut().zip(mailboxes).skip(1) {
        let features = exchange(
            guest,
            guest_log,
            mailbox,
            Request {
                binding: 1,
                epoch: 1,
                transaction: 1,
                command: Command::Hello,
            },
        )?
        .features
        .ok_or("Native volume HELLO omitted features")?;
        client.configure(guest, mailbox, guest_log, features)?;
        volumes.push((
            client
                .policy
                .clone()
                .ok_or("missing Native storage policy")?,
            client
                .partition
                .clone()
                .ok_or("missing discovered partition")?,
            client.block.take().ok_or("Native initiator missing")?,
        ));
    }
    let mailbox = mailboxes
        .first()
        .ok_or("Native storage requires a mailbox")?;
    let (sender, receiver) = hyper_os::channel::create_pair().map_err(show)?;
    std::thread::scope(|scope| -> Result<bool> {
        let worker = std::thread::Builder::new()
            .spawn_scoped(scope, move || {
                let result = filesystem::attach(provider, volumes);
                let _ = sender.as_byte_channel().try_send(b"done");
                result
            })
            .map_err(show)?;
        let pending = wait_mount(guest, guest_log, mailbox, &receiver);
        if pending.is_err() {
            let _ = vm::request_stop(guest.machine.as_handle_ref());
        }
        let result = worker
            .join()
            .map_err(|_| "Native volume attachment worker panicked".to_string())?;
        pending?;
        result
    })
}

fn exchange(
    guest: &mut InstalledGuest,
    guest_log: &mut GuestLog,
    mailbox: &Mailbox,
    request: Request,
) -> Result<Reply> {
    let mut record = [0; MAX_RECORD];
    let length = request.encode(&mut record).map_err(show)?;
    mailbox.send(&record[..length]).map_err(show)?;
    let limit = deadline(60)?;
    loop {
        if !pump_guest(guest, guest_log)? {
            return Err("backend stopped during configuration".into());
        }
        match mailbox.receive(&mut record) {
            Ok(length) => {
                let reply = Reply::decode(&record[..length], request).map_err(show)?;
                return if reply.status == Status::Success {
                    Ok(reply)
                } else {
                    Err(format!(
                        "backend refused {:?}: {:?}",
                        request.command, reply.status
                    ))
                };
            }
            Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => {}
            Err(error) => return Err(show(error)),
        }
        check_deadline(limit)?;
        let items = [
            guest.output.wait_item(),
            WaitItem::new(
                guest.machine.as_handle_ref(),
                ObjectSignals::<VirtualMachineObject>::POWER_REQUEST
                    .union(ObjectSignals::<VirtualMachineObject>::VCPU_TERMINATED),
            ),
            WaitItem::new(
                mailbox.as_handle_ref(),
                ObjectSignals::<GuestMailboxObject>::READABLE
                    .union(ObjectSignals::<GuestMailboxObject>::PEER_CLOSED),
            ),
        ];
        let event = wait::wait_many(&items, limit).map_err(show)?;
        if event.index == 2
            && ObjectSignals::<GuestMailboxObject>::PEER_CLOSED.is_present_in(event.observed)
        {
            return Err("backend control channel closed during configuration".into());
        }
    }
}

fn wait_mount(
    guest: &mut InstalledGuest,
    guest_log: &mut GuestLog,
    mailbox: &Mailbox,
    receiver: &OwnedHandle<ByteChannelObject>,
) -> Result<()> {
    let limit = deadline(crate::contracts::io::READY_TIMEOUT_SECONDS)?;
    loop {
        if !pump_guest(guest, guest_log)? {
            return Err("backend stopped during mount".into());
        }
        // Observe the durable worker result before serial readiness/deadline.
        match receiver.as_byte_channel().try_receive(&mut [0; 4]) {
            Ok(_) | Err(hyper_os::Error::Status(hyper_os::Status::PEER_CLOSED)) => return Ok(()),
            Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => {}
            Err(error) => return Err(show(error)),
        }
        check_deadline(limit)?;
        let items = [
            guest.output.wait_item(),
            WaitItem::new(
                guest.machine.as_handle_ref(),
                ObjectSignals::<VirtualMachineObject>::POWER_REQUEST
                    .union(ObjectSignals::<VirtualMachineObject>::VCPU_TERMINATED),
            ),
            WaitItem::new(
                mailbox.as_handle_ref(),
                ObjectSignals::<GuestMailboxObject>::READABLE
                    .union(ObjectSignals::<GuestMailboxObject>::PEER_CLOSED),
            ),
            WaitItem::new(
                receiver.as_handle_ref(),
                ObjectSignals::<ByteChannelObject>::READABLE
                    .union(ObjectSignals::<ByteChannelObject>::PEER_CLOSED),
            ),
        ];
        match wait::wait_many(&items, limit).map_err(show)?.index {
            2 => return Err("unexpected backend control event during mount".into()),
            3 => return Ok(()),
            _ => {}
        }
    }
}
