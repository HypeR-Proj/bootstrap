// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Claimed network hardware and the firmware borrowed by the I/O guest image.

use super::{Result, show};
use hyper_io_runtime::{device_policy::Policy, pci, rp1};
use hyper_os::device;
use hyper_os::handle::{
    DeviceAssignmentAuthorityObject, HandleRef, OwnedHandle, PhysicalDeviceObject,
};
use hyper_vm_image::guest_fdt::io::{MmioDevice, PciHost};
use hyper_vm_support::io_guest::{
    PHYSICAL_NET_IRQ, PHYSICAL_NET_MMIO, PHYSICAL_PCI_IRQ, PHYSICAL_PCI_MMIO, PhysicalAssignment,
};

pub(super) enum PhysicalNetwork {
    Absent,
    Virtio {
        device: OwnedHandle<PhysicalDeviceObject>,
        node: MmioDevice,
    },
    Pci {
        firmware: rp1::Projection,
        assignment: pci::Assignment,
    },
}

impl PhysicalNetwork {
    pub(super) fn claim(
        authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
        policy: Option<&Policy>,
    ) -> Result<Self> {
        match policy {
            None => Ok(Self::Absent),
            Some(policy) if policy.profile() == device::Profile::PciFunction => {
                let assignment = pci::Assignment::claim(
                    authority,
                    policy.identity(),
                    PHYSICAL_PCI_MMIO,
                    PHYSICAL_PCI_IRQ,
                )
                .map_err(show)?;
                // This board's function is RP1. Only its firmware description is
                // board-specific; ECAM/BAR/MSI transport is generic PCI metadata.
                if !matches!(
                    policy.identity(),
                    device::FirmwareIdentity::PciId {
                        vendor: 0x1de4,
                        device: 1
                    }
                ) {
                    return Err("no board firmware projection for selected PCI function".into());
                }
                let firmware = rp1::describe(authority, assignment.bars()).map_err(show)?;
                Ok(Self::Pci {
                    firmware,
                    assignment,
                })
            }
            Some(policy) => {
                let device = device::claim_matching(authority, policy.profile(), policy.identity())
                    .map_err(show)?;
                let info = device::profile_info(device.as_handle_ref()).map_err(show)?;
                let window = device::resource_info(device.as_handle_ref(), 0).map_err(show)?;
                if info.profile != device::Profile::VirtioMmioNet
                    || info.resource_count != 1
                    || window.kind != 1
                    || window.offset != 0
                    || !(0x200..=4096).contains(&window.length)
                {
                    return Err("invalid physical network resource".into());
                }
                Ok(Self::Virtio {
                    device,
                    node: MmioDevice {
                        base: PHYSICAL_NET_MMIO,
                        size: window.length,
                        irq: PHYSICAL_NET_IRQ,
                    },
                })
            }
        }
    }

    /// The PCI node view borrows local projection storage and is usable only
    /// while the caller builds the image inside this callback.
    pub(super) fn with_description<R>(
        &self,
        describe: impl FnOnce(Option<MmioDevice>, Option<PciHost<'_>>) -> R,
    ) -> R {
        match self {
            Self::Absent => describe(None, None),
            Self::Virtio { node, .. } => describe(Some(*node), None),
            Self::Pci {
                firmware,
                assignment,
            } => firmware.with_nodes(|nodes| describe(None, Some(assignment.host(nodes)))),
        }
    }

    pub(super) fn assignment(&self) -> Option<PhysicalAssignment<'_>> {
        match self {
            Self::Absent => None,
            Self::Virtio { device, .. } => Some(PhysicalAssignment {
                device: device.as_handle_ref(),
                base: PHYSICAL_NET_MMIO,
                irq: PHYSICAL_NET_IRQ,
            }),
            Self::Pci { assignment, .. } => Some(PhysicalAssignment {
                device: assignment.device().as_handle_ref(),
                base: PHYSICAL_PCI_MMIO,
                irq: PHYSICAL_PCI_IRQ,
            }),
        }
    }
}
