// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Transfer consumers at a quiescent boundary while the VM keeps running.

use super::broker::Broker;
use super::{Result, deadline, drain, pump_guest, show};
use hyper_io_runtime::guest::install::InstalledGuest;
use hyper_io_runtime::guest_log::GuestLog;
use hyper_io_runtime::handoff::{self, Description, Kind, Slot, WorkerMode};
use hyper_io_runtime::mmio::{self as worker, Worker};
use hyper_os::capability_channel::{CapabilityChannel, CapabilityDisposition};
use hyper_os::guest_io::Mailbox;
use hyper_os::handle::*;
use hyper_os::memory::WritableVmo;
use hyper_os::virtual_serial::{self, Output};
use hyper_os::vm;
use hyper_os::wait::{self, ObjectSignals, WaitItem};
use std::sync::Arc;

pub(super) struct Owner {
    pub guest: InstalledGuest,
    pub own_memory: OwnedHandle<GuestMemoryObject>,
    pub hotplug_interrupt: OwnedHandle<GuestInterruptObject>,
    pub pcie: hyper_io_runtime::pcie::RootPort,
    pub log: GuestLog,
    pub worker: Option<Worker>,
    pub mode: WorkerMode,
    pub physical: OwnedHandle<PhysicalDeviceObject>,
    pub network: Option<OwnedHandle<PhysicalDeviceObject>>,
    pub native_memory: Vec<OwnedHandle<GuestMemoryObject>>,
    pub native_mailboxes: Vec<Mailbox>,
    pub broker: Option<Broker>,
}
impl Owner {
    fn description(&self) -> Result<Description> {
        let mut slots = vec![slot(Kind::Machine, self.guest.machine.as_handle_ref())?];
        for cpu in &self.guest.cpus {
            slots.push(slot(Kind::Cpu, cpu.as_handle_ref())?);
        }
        slots.push(Slot {
            kind: Kind::Serial,
            rights: virtual_serial::OUTPUT_RIGHTS.bits(),
        });
        slots.push(Slot {
            kind: Kind::SerialBuffer,
            rights: virtual_serial::BUFFER_RIGHTS.bits(),
        });
        slots.push(slot(Kind::OwnMemory, self.own_memory.as_handle_ref())?);
        slots.push(slot(
            Kind::HotplugInterrupt,
            self.hotplug_interrupt.as_handle_ref(),
        )?);
        slots.push(slot(Kind::StorageDevice, self.physical.as_handle_ref())?);
        if let Some(network) = &self.network {
            slots.push(slot(Kind::NetworkDevice, network.as_handle_ref())?);
        }
        if let Some(broker) = &self.broker {
            slots.push(slot(Kind::Broker, broker.endpoint.as_handle_ref())?);
        }
        for memory in &self.native_memory {
            slots.push(slot(Kind::NativeMemory, memory.as_handle_ref())?);
        }
        for mailbox in &self.native_mailboxes {
            slots.push(slot(Kind::NativeMailbox, mailbox.as_handle_ref())?);
        }
        if let Some(broker) = &self.broker {
            for client in &broker.slots {
                slots.push(slot(
                    Kind::BrokerMailbox,
                    client
                        .mailbox
                        .as_ref()
                        .ok_or("uninstalled broker mailbox")?
                        .as_handle_ref(),
                )?);
            }
        }
        let description = Description {
            version: 2,
            memory_base: hyper_io_runtime::guest::install::RAM_BASE,
            memory_size: hyper_io_runtime::guest::install::RAM_BYTES,
            name: self.guest.name().into(),
            image: self.guest.image().into(),
            worker: self.mode,
            cpus: self.guest.cpus.len(),
            native: self.native_memory.len(),
            network: self.network.is_some(),
            broker: self.broker.is_some(),
            clients: self
                .broker
                .as_ref()
                .map(|broker| {
                    broker
                        .slots
                        .iter()
                        .map(|slot| slot.policy.clone())
                        .collect()
                })
                .unwrap_or_default(),
            slots,
        };
        description.validate().map_err(show)?;
        Ok(description)
    }
    fn stop(mut self, outcome: Result<()>) -> Result<()> {
        let stopped = vm::request_stop(self.guest.machine.as_handle_ref())
            .map_err(show)
            .and_then(|()| {
                vm::wait_terminated(self.guest.machine.as_handle_ref(), deadline(20)?).map_err(show)
            });
        let worker = self
            .worker
            .take()
            .map(|worker| worker.stop().map_err(show))
            .transpose()
            .map(|_| ());
        let drained = drain(&mut self.guest, &mut self.log);
        outcome.and(stopped).and(worker).and(drained)
    }
}
fn slot<T: TypedObject>(kind: Kind, handle: HandleRef<'_, T>) -> Result<Slot> {
    Ok(Slot {
        kind,
        rights: handle.info().map_err(show)?.rights.bits(),
    })
}

/// No candidate, failed preparation, or a closed handoff channel retires the
/// bootstrap provider. Only system init owns the eventual process stop policy.
pub(super) fn serve(mut owner: Owner, mut endpoint: Option<CapabilityChannel>) -> Result<()> {
    let prepared = endpoint
        .as_ref()
        .map(|_| prepare_description(&owner))
        .transpose();
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!(
                "{}cannot prepare adoption; retaining backend: {error}",
                crate::logging::prefix("io-runtime")
            );
            endpoint = None;
            None
        }
    };
    let mut described = false;
    loop {
        match pump_guest(&mut owner.guest, &mut owner.log) {
            Ok(true) => (),
            Ok(false) => return owner.stop(Ok(())),
            Err(error) => return owner.stop(Err(error)),
        }
        if let Some(channel) = &endpoint
            && !described
        {
            let result = (|| -> hyper_os::Result<()> {
                let (_, snapshot) = prepared.as_ref().ok_or(hyper_os::Error::MissingHandle)?;
                let mut offer = [CapabilityDisposition::duplicate(
                    snapshot.as_handle_ref(),
                    RightsOffer::Exact(Rights::READ.union(Rights::INSPECT).union(Rights::TRANSFER)),
                )?];
                channel.try_send(handoff::DESCRIBE_MESSAGE, &mut offer)
            })();
            match result {
                Ok(()) => described = true,
                Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => (),
                Err(error) => {
                    eprintln!(
                        "{}adoption unavailable: {error:?}",
                        crate::logging::prefix("io-runtime")
                    );
                    endpoint = None;
                }
            }
        }
        let mut items = vec![
            owner.guest.output.wait_item(),
            WaitItem::new(
                owner.guest.machine.as_handle_ref(),
                ObjectSignals::<VirtualMachineObject>::POWER_REQUEST
                    .union(ObjectSignals::<VirtualMachineObject>::VCPU_TERMINATED),
            ),
        ];
        if let Some(channel) = &endpoint {
            items.push(WaitItem::new(
                channel.as_handle_ref(),
                ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
                    .union(ObjectSignals::<CapabilityChannelObject>::PEER_CLOSED),
            ));
        }
        let event = match wait::wait_many(&items, hyper_os::DEADLINE_INFINITE) {
            Ok(event) => event,
            Err(error) => return owner.stop(Err(show(error))),
        };
        drop(items);
        if event.index != 2 {
            continue;
        }
        if ObjectSignals::<CapabilityChannelObject>::PEER_CLOSED.is_present_in(event.observed) {
            endpoint = None;
            continue;
        }
        if !described {
            continue;
        }
        let Some(channel) = endpoint.take() else {
            continue;
        };
        // First stop the physical MMIO/IRQ consumer; Arc owners become unique.
        if let Some(worker) = owner.worker.take() {
            match worker.quiesce() {
                Ok(pcie) => owner.pcie = pcie,
                Err(error) => return owner.stop(Err(show(error))),
            }
        }
        let Some((description, _)) = &prepared else {
            // No live resource is touched if metadata could not be prepared.
            continue;
        };
        match transfer(owner, description, &channel) {
            Ok(None) => {
                println!(
                    "{}running backend transferred to system",
                    crate::logging::prefix("io-runtime")
                );
                // Adoption is not retirement. Keep this process quiet until
                // system init explicitly stops and joins its bootstrap group.
                loop {
                    std::thread::park();
                }
            }
            Ok(Some(restored)) => owner = restored,
            Err(error) => return Err(error),
        }
    }
}

fn prepare_description(owner: &Owner) -> Result<(Description, WritableVmo)> {
    let description = owner.description()?;
    let bytes = serde_json::to_vec(&description).map_err(show)?;
    if bytes.len() > handoff::MAX_DESCRIPTION {
        return Err("I/O handoff description too large".into());
    }
    let memory =
        WritableVmo::create((bytes.len() as u64 + 8).div_ceil(4096) * 4096).map_err(show)?;
    memory
        .write_all_at(0, &(bytes.len() as u64).to_le_bytes())
        .map_err(show)?;
    memory.write_all_at(8, &bytes).map_err(show)?;
    Ok((description, memory))
}

struct Transfer {
    machine: Option<OwnedHandle<VirtualMachineObject>>,
    cpus: Vec<Option<OwnedHandle<VirtualCpuObject>>>,
    serial: Option<OwnedHandle<VirtualSerialObject>>,
    ring: Option<OwnedHandle<VmoObject>>,
    original_output: Option<Output>,
    own_memory: Option<OwnedHandle<GuestMemoryObject>>,
    hotplug_interrupt: Option<OwnedHandle<GuestInterruptObject>>,
    physical: Option<OwnedHandle<PhysicalDeviceObject>>,
    network: Option<OwnedHandle<PhysicalDeviceObject>>,
    broker: Option<OwnedHandle<CapabilityChannelObject>>,
    native_memory: Vec<Option<OwnedHandle<GuestMemoryObject>>>,
    native_mailboxes: Vec<Option<OwnedHandle<GuestMailboxObject>>>,
    broker_mailboxes: Vec<Option<OwnedHandle<GuestMailboxObject>>>,
}
impl Drop for Transfer {
    fn drop(&mut self) {
        // A failed post-quiescence reconstruction must stop execution before
        // dropping its last live owner. Successful MOVE leaves this empty.
        if let Some(machine) = &self.machine {
            let _ = vm::request_stop(machine.as_handle_ref());
        }
    }
}

fn transfer(
    owner: Owner,
    description: &Description,
    channel: &CapabilityChannel,
) -> Result<Option<Owner>> {
    if Arc::strong_count(&owner.guest.machine) != 1
        || owner
            .guest
            .cpus
            .iter()
            .any(|cpu| Arc::strong_count(cpu) != 1)
    {
        return owner
            .stop(Err("I/O consumers did not quiesce".into()))
            .map(|()| None);
    }
    let Owner {
        guest,
        own_memory,
        hotplug_interrupt,
        pcie,
        log,
        mode,
        physical,
        network,
        native_memory,
        native_mailboxes,
        broker,
        ..
    } = owner;
    let InstalledGuest {
        name,
        image,
        machine,
        cpus,
        output,
    } = guest;
    let paused = match output.handoff_state() {
        Ok(paused) => paused,
        Err(error) => {
            let mut owner = Owner {
                guest: InstalledGuest {
                    name,
                    image,
                    machine,
                    cpus,
                    output,
                },
                own_memory,
                hotplug_interrupt,
                pcie,
                log,
                mode,
                physical,
                network,
                native_memory,
                native_mailboxes,
                broker,
                worker: None,
            };
            eprintln!(
                "{}cannot pause output for adoption: {error:?}",
                crate::logging::prefix("io-runtime")
            );
            owner.worker = resume_worker(&owner)?;
            return Ok(Some(owner));
        }
    };
    let cursor = paused.cursor;
    let (broker, broker_mailboxes) = match broker {
        Some(broker) => (
            Some(broker.endpoint),
            broker
                .slots
                .into_iter()
                .map(|slot| slot.mailbox.map(Mailbox::into_handle))
                .collect(),
        ),
        None => (None, Vec::new()),
    };
    let mut state = Transfer {
        machine: Some(Arc::try_unwrap(machine).map_err(|_| "I/O machine remained shared")?),
        cpus: cpus
            .into_iter()
            .map(|cpu| {
                Arc::try_unwrap(cpu)
                    .map(Some)
                    .map_err(|_| "I/O vCPU remained shared".into())
            })
            .collect::<Result<_>>()?,
        serial: Some(paused.serial),
        ring: Some(paused.memory),
        original_output: Some(output),
        own_memory: Some(own_memory),
        hotplug_interrupt: Some(hotplug_interrupt),
        physical: Some(physical),
        network,
        broker,
        native_memory: native_memory.into_iter().map(Some).collect(),
        native_mailboxes: native_mailboxes
            .into_iter()
            .map(|mailbox| Some(mailbox.into_handle()))
            .collect(),
        broker_mailboxes,
    };
    let bytes = handoff::encode_commit(cursor, log.line_start(), &pcie);
    let limit = deadline(10)?;
    let mut index = 0;
    let mut dispositions = Vec::new();
    macro_rules! offer {
        ($source:expr) => {{
            let rights = description
                .slots
                .get(index)
                .and_then(|slot| Rights::from_bits(slot.rights))
                .ok_or("invalid I/O offer rights")?;
            dispositions.push(
                CapabilityDisposition::move_handle($source, RightsOffer::Exact(rights))
                    .map_err(show)?,
            );
            index += 1;
        }};
    }
    offer!(&mut state.machine);
    for cpu in &mut state.cpus {
        offer!(cpu);
    }
    offer!(&mut state.serial);
    offer!(&mut state.ring);
    offer!(&mut state.own_memory);
    offer!(&mut state.hotplug_interrupt);
    offer!(&mut state.physical);
    if state.network.is_some() {
        offer!(&mut state.network);
    }
    if state.broker.is_some() {
        offer!(&mut state.broker);
    }
    for memory in &mut state.native_memory {
        offer!(memory);
    }
    for mailbox in &mut state.native_mailboxes {
        offer!(mailbox);
    }
    for mailbox in &mut state.broker_mailboxes {
        offer!(mailbox);
    }
    if index != description.slots.len() {
        return Err("I/O offer changed during quiescence".into());
    }
    let result = loop {
        match channel.try_send(&bytes, &mut dispositions) {
            Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => {
                if let Err(error) = wait::wait_many(
                    &[WaitItem::new(
                        channel.as_handle_ref(),
                        ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
                            .union(ObjectSignals::<CapabilityChannelObject>::PEER_CLOSED),
                    )],
                    limit,
                ) {
                    break Err(error);
                }
            }
            result => break result,
        }
    };
    drop(dispositions);
    if result.is_ok() {
        return Ok(None);
    }
    eprintln!(
        "{}adoption rejected; retaining running backend: {result:?}",
        crate::logging::prefix("io-runtime")
    );
    let mut owner = Owner {
        guest: InstalledGuest {
            name,
            image,
            machine: Arc::new(
                state
                    .machine
                    .take()
                    .ok_or("missing machine after rejected MOVE")?,
            ),
            cpus: state
                .cpus
                .iter_mut()
                .map(|cpu| {
                    cpu.take()
                        .map(Arc::new)
                        .ok_or("missing vCPU after rejected MOVE".into())
                })
                .collect::<Result<_>>()?,
            output: state
                .original_output
                .take()
                .ok_or("missing original output mapping")?,
        },
        own_memory: state.own_memory.take().ok_or("missing own RAM")?,
        hotplug_interrupt: state
            .hotplug_interrupt
            .take()
            .ok_or("missing hotplug interrupt")?,
        pcie,
        log,
        mode,
        worker: None,
        physical: state.physical.take().ok_or("missing storage device")?,
        network: state.network.take(),
        native_memory: state
            .native_memory
            .iter_mut()
            .map(|memory| memory.take().ok_or("missing Native memory".into()))
            .collect::<Result<_>>()?,
        native_mailboxes: state
            .native_mailboxes
            .iter_mut()
            .map(|mailbox| {
                mailbox
                    .take()
                    .map(Mailbox::from_handle)
                    .ok_or("missing Native mailbox".into())
            })
            .collect::<Result<_>>()?,
        broker: state
            .broker
            .take()
            .map(|endpoint| -> Result<Broker> {
                Ok(Broker {
                    endpoint,
                    slots: description
                        .clients
                        .iter()
                        .cloned()
                        .zip(&mut state.broker_mailboxes)
                        .map(|(policy, mailbox)| super::broker::ClientSlot {
                            policy,
                            mailbox: mailbox.take().map(Mailbox::from_handle),
                        })
                        .collect(),
                })
            })
            .transpose()?,
    };
    owner.worker = resume_worker(&owner)?;
    Ok(Some(owner))
}
fn resume_worker(owner: &Owner) -> Result<Option<Worker>> {
    match owner.mode {
        WorkerMode::None => worker::resume_bus(&owner.guest, owner.pcie.clone())
            .map(Some)
            .map_err(show),
        WorkerMode::Sdhci => worker::resume(&owner.physical, &owner.guest, owner.pcie.clone())
            .map(Some)
            .map_err(show),
        #[cfg(feature = "userspace-device-test")]
        WorkerMode::VirtioTest => {
            worker::resume_virtio_test(&owner.physical, &owner.guest, owner.pcie.clone())
                .map(Some)
                .map_err(show)
        }
        #[cfg(not(feature = "userspace-device-test"))]
        WorkerMode::VirtioTest => Err("test device consumer unavailable".into()),
    }
}
