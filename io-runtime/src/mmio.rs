// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! One owner for physical-device and empty `PCIe` root-port exits.
//! Storage waits cannot block Linux device discovery, and no two consumers
//! race on the same vCPU MMIO endpoint.

use crate::guest::install::{InstalledGuest, PHYSICAL_MMIO};
use crate::pcie::{self, RootPort};
use hyper_os::handle::{
    ByteChannelObject, OwnedHandle, PhysicalDeviceObject, VirtualCpuObject, VirtualMachineObject,
};
use hyper_os::wait::{ObjectSignals, RegistrationId, WaitSet};
use hyper_os::{Error, Result, Status, channel, device, vm};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub use crate::sdhci::registers::DEVICE_COOKIE;
use crate::sdhci::registers::{line_asserted, request_offset};

pub struct Worker {
    // Closing this endpoint is a durable stop notification, even if the worker
    // is between servicing a request and entering WaitSet::wait.
    stop: Option<OwnedHandle<ByteChannelObject>>,
    thread: Option<JoinHandle<Result<Exit>>>,
}
impl Worker {
    /// Stop accepting exits, request VM shutdown, and join the worker. No VM
    /// lifecycle lock may be held by the caller while joining this thread.
    pub fn stop(mut self) -> Result<()> {
        self.stop.take();
        match self.thread.take() {
            Some(thread) => {
                thread
                    .join()
                    .map_err(|_| Error::Status(Status::INTERNAL))??;
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Joins the MMIO consumer at a request boundary without stopping the VM.
    /// Pending exits and IRQ levels remain in their kernel objects for the
    /// successor. A failure still follows the normal stop-on-error path.
    pub fn quiesce(mut self) -> Result<RootPort> {
        self.stop
            .as_ref()
            .ok_or(Error::MissingHandle)?
            .as_byte_channel()
            .send(&[QUIESCE])?;
        let thread = self.thread.take().ok_or(Error::MissingHandle)?;
        match thread
            .join()
            .map_err(|_| Error::Status(Status::INTERNAL))??
        {
            Exit::Quiesced(pcie) => Ok(pcie),
            Exit::Stopped => Err(Error::Status(Status::PEER_CLOSED)),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Never block a VM retirement path in Drop. The worker retains all
        // capabilities until it observes closure and requests shutdown.
        self.stop.take();
    }
}

const QUIESCE: u8 = 1;
enum Exit {
    Stopped,
    Quiesced(RootPort),
}

struct MachineGuard {
    machine: Arc<OwnedHandle<VirtualMachineObject>>,
    stop_on_drop: bool,
}
impl Drop for MachineGuard {
    fn drop(&mut self) {
        if self.stop_on_drop {
            let _ = vm::request_stop(self.machine.as_handle_ref());
        }
    }
}

struct Pump {
    physical: Option<OwnedHandle<PhysicalDeviceObject>>,
    machine: MachineGuard,
    cpus: Vec<Arc<OwnedHandle<VirtualCpuObject>>>,
    _stop: OwnedHandle<ByteChannelObject>,
    waits: WaitSet,
    stop_id: RegistrationId,
    machine_id: RegistrationId,
    irq_id: Option<RegistrationId>,
    pcie: RootPort,
    cpu_ids: Vec<RegistrationId>,
    irq_armed: bool,
    mode: Mode,
}

/// The caller must first register `PHYSICAL_MMIO..+65536` with `DEVICE_COOKIE`,
/// and must not run the guest before this function succeeds.
pub fn start(
    physical: &OwnedHandle<PhysicalDeviceObject>,
    guest: &InstalledGuest,
) -> Result<Worker> {
    start_mode(
        Some(physical),
        guest,
        Mode::Sdhci,
        true,
        RootPort::default(),
    )
}

/// Starts a new consumer for an already assigned, running SDHCI device.
/// The predecessor must have completed `Worker::quiesce` first. This does not
/// reset hardware or revalidate power-on register contents.
pub fn resume(
    physical: &OwnedHandle<PhysicalDeviceObject>,
    guest: &InstalledGuest,
    pcie: RootPort,
) -> Result<Worker> {
    start_mode(Some(physical), guest, Mode::Sdhci, false, pcie)
}

#[cfg(feature = "userspace-device-test")]
pub fn start_virtio_test(
    physical: &OwnedHandle<PhysicalDeviceObject>,
    guest: &InstalledGuest,
) -> Result<Worker> {
    start_mode(
        Some(physical),
        guest,
        Mode::VirtioTest,
        true,
        RootPort::default(),
    )
}

#[cfg(feature = "userspace-device-test")]
pub fn resume_virtio_test(
    physical: &OwnedHandle<PhysicalDeviceObject>,
    guest: &InstalledGuest,
    pcie: RootPort,
) -> Result<Worker> {
    start_mode(Some(physical), guest, Mode::VirtioTest, false, pcie)
}

/// The empty `PCIe` slot also needs an independent consumer during disk waits.
pub fn start_bus(guest: &InstalledGuest) -> Result<Worker> {
    resume_bus(guest, RootPort::default())
}
pub fn resume_bus(guest: &InstalledGuest, pcie: RootPort) -> Result<Worker> {
    start_mode(None, guest, Mode::BusOnly, false, pcie)
}

#[derive(Clone, Copy)]
enum Mode {
    BusOnly,
    Sdhci,
    #[cfg(feature = "userspace-device-test")]
    VirtioTest,
}
impl Mode {
    fn validate(self, physical: &OwnedHandle<PhysicalDeviceObject>) -> Result<()> {
        let valid = match self {
            Self::BusOnly => false,
            Self::Sdhci => device::mmio_read(physical.as_handle_ref(), 0x40, 4)? & (1 << 28) != 0,
            #[cfg(feature = "userspace-device-test")]
            Self::VirtioTest => {
                device::mmio_read(physical.as_handle_ref(), 0, 4)? == 0x74726976
                    && device::mmio_read(physical.as_handle_ref(), 4, 4)? == 2
                    && device::mmio_read(physical.as_handle_ref(), 8, 4)? == 8
                    && device::mmio_read(physical.as_handle_ref(), 0x70, 4)? == 0
            }
        };
        if valid {
            Ok(())
        } else {
            Err(Error::Status(Status::NOT_SUPPORTED))
        }
    }
    fn asserted(self, physical: &OwnedHandle<PhysicalDeviceObject>) -> Result<bool> {
        match self {
            Self::BusOnly => Ok(false),
            Self::Sdhci => {
                let status = device::mmio_read(physical.as_handle_ref(), 0x30, 4)? as u32;
                let enable = device::mmio_read(physical.as_handle_ref(), 0x38, 4)? as u32;
                Ok(line_asserted(status, enable))
            }
            #[cfg(feature = "userspace-device-test")]
            Self::VirtioTest => Ok(device::mmio_read(physical.as_handle_ref(), 0x60, 4)? & 3 != 0),
        }
    }
}

fn start_mode(
    physical: Option<&OwnedHandle<PhysicalDeviceObject>>,
    guest: &InstalledGuest,
    mode: Mode,
    validate: bool,
    pcie: RootPort,
) -> Result<Worker> {
    if guest.cpus.is_empty() || guest.cpus.len() > 8 {
        return Err(Error::Status(Status::NOT_SUPPORTED));
    }
    let physical = physical
        .map(|physical| physical.duplicate(physical.info()?.rights))
        .transpose()?;
    // Share the same process-local capabilities. Installed VM/vCPU handles
    // intentionally cannot be duplicated through the Native capability API.
    let machine = MachineGuard {
        machine: Arc::clone(&guest.machine),
        stop_on_drop: true,
    };
    // Even a read may have device side effects. Only inspect capabilities after
    // installation has retained the VM backing and activated the DMA lease.
    if validate {
        mode.validate(physical.as_ref().ok_or(Error::MissingHandle)?)?;
    }
    let mut cpus = Vec::new();
    cpus.try_reserve_exact(guest.cpus.len())
        .map_err(|_| Error::Status(Status::NO_MEMORY))?;
    for cpu in &guest.cpus {
        cpus.push(Arc::clone(cpu));
    }
    let (stop, receiver) = channel::create_pair()?;
    let waits = WaitSet::new(cpus.len() + 3)?;
    let stop_id = waits.add(
        receiver.as_handle_ref(),
        ObjectSignals::<ByteChannelObject>::PEER_CLOSED
            .union(ObjectSignals::<ByteChannelObject>::READABLE),
    )?;
    let machine_id = waits.add(
        machine.machine.as_handle_ref(),
        ObjectSignals::<VirtualMachineObject>::TERMINATED,
    )?;
    let irq_id = physical
        .as_ref()
        .map(|physical| {
            waits.add(
                physical.as_handle_ref(),
                ObjectSignals::<PhysicalDeviceObject>::READABLE,
            )
        })
        .transpose()?;
    let mut cpu_ids = Vec::new();
    cpu_ids
        .try_reserve_exact(cpus.len())
        .map_err(|_| Error::Status(Status::NO_MEMORY))?;
    for cpu in &cpus {
        cpu_ids.push(waits.add(
            cpu.as_handle_ref(),
            ObjectSignals::<VirtualCpuObject>::MMIO_REQUEST,
        )?);
    }
    let pump = Pump {
        physical,
        machine,
        cpus,
        _stop: receiver,
        waits,
        stop_id,
        machine_id,
        irq_id,
        pcie,
        cpu_ids,
        irq_armed: true,
        mode,
    };
    let thread = thread::Builder::new()
        .name("io-mmio".into())
        .stack_size(64 * 1024)
        .spawn(move || pump.run())
        .map_err(|_| Error::Status(Status::NO_MEMORY))?;
    Ok(Worker {
        stop: Some(stop),
        thread: Some(thread),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Line {
    Asserted,
    Idle,
    Changed,
}

impl Pump {
    fn reconcile(&self) -> Result<Line> {
        let Some(physical) = self.physical.as_ref() else {
            return Ok(Line::Idle);
        };
        for _ in 0..2 {
            let sequence = device::irq_pending(physical.as_handle_ref())?;
            let asserted = self.mode.asserted(physical)?;
            match device::irq_complete(physical.as_handle_ref(), sequence, asserted) {
                Ok(()) => return Ok(if asserted { Line::Asserted } else { Line::Idle }),
                Err(Error::Status(Status::BUSY)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Line::Changed)
    }

    fn update_irq(&mut self) -> Result<()> {
        let line = self.reconcile()?;
        if !self.irq_armed && line != Line::Asserted {
            if let Some(id) = self.irq_id {
                self.waits.rearm(id)?;
            }
            self.irq_armed = true;
        }
        Ok(())
    }

    fn mmio(&mut self, index: usize) -> Result<()> {
        if let Some(request) = vm::pending_mmio(self.cpus[index].as_handle_ref())? {
            let completion = if request.device.get() == pcie::ECAM_COOKIE {
                let (bus, device, function, offset) =
                    pcie::decode_ecam(request.address).ok_or(Error::InvalidResponse)?;
                let root = bus == 0 && device == 0 && function == 0;
                match request.operation {
                    vm::MmioOperation::Read => vm::MmioCompletion::Read(
                        if root {
                            self.pcie.read(offset, request.width as u8)
                        } else {
                            pcie::absent(request.width as u8)
                        }
                        .ok_or(Error::InvalidResponse)?,
                    ),
                    vm::MmioOperation::Write(value) => {
                        if root {
                            self.pcie
                                .write(offset, request.width as u8, value)
                                .ok_or(Error::InvalidResponse)?;
                        }
                        vm::MmioCompletion::Write
                    }
                }
            } else if request.device.get() == pcie::MEMORY_COOKIE {
                // Bootstrap never installs an endpoint. Absent memory decodes
                // are harmless, and the successor retains the same aperture.
                match request.operation {
                    vm::MmioOperation::Read => vm::MmioCompletion::Read(
                        pcie::absent(request.width as u8).ok_or(Error::InvalidResponse)?,
                    ),
                    vm::MmioOperation::Write(_) => vm::MmioCompletion::Write,
                }
            } else {
                let physical = self.physical.as_ref().ok_or(Error::InvalidResponse)?;
                let offset = request_offset(request, PHYSICAL_MMIO)?;
                match request.operation {
                    vm::MmioOperation::Read => vm::MmioCompletion::Read(device::mmio_read(
                        physical.as_handle_ref(),
                        offset,
                        request.width,
                    )?),
                    vm::MmioOperation::Write(value) => {
                        device::mmio_write(physical.as_handle_ref(), offset, request.width, value)?;
                        vm::MmioCompletion::Write
                    }
                }
            };
            self.update_irq()?;
            vm::complete_mmio(self.cpus[index].as_handle_ref(), request.id, completion)?;
        }
        self.waits.rearm(self.cpu_ids[index])
    }

    fn run(mut self) -> Result<Exit> {
        let result = self.service();
        if matches!(result, Ok(Exit::Quiesced(_))) {
            // All register accesses and MMIO completions have finished. Only
            // this path disarms retirement; an error or unwind still stops VM.
            self.machine.stop_on_drop = false;
        }
        result
    }

    fn service(&mut self) -> Result<Exit> {
        self.update_irq()?;
        loop {
            let event = self.waits.wait(u64::MAX)?;
            if event.registration == self.machine_id {
                return Ok(Exit::Stopped);
            }
            if event.registration == self.stop_id {
                if ObjectSignals::<ByteChannelObject>::PEER_CLOSED.is_present_in(event.signals) {
                    return Ok(Exit::Stopped);
                }
                let mut command = [0; 1];
                return match self._stop.as_byte_channel().try_receive(&mut command) {
                    Ok(1) if command[0] == QUIESCE => Ok(Exit::Quiesced(self.pcie.clone())),
                    Ok(_) => Err(Error::InvalidResponse),
                    Err(Error::Status(Status::PEER_CLOSED)) => Ok(Exit::Stopped),
                    Err(error) => Err(error),
                };
            }
            if Some(event.registration) == self.irq_id {
                self.irq_armed = false;
                self.update_irq()?;
            } else if let Some(index) = self.cpu_ids.iter().position(|id| *id == event.registration)
            {
                self.mmio(index)?;
            } else {
                return Err(Error::InvalidResponse);
            }
        }
    }
}
