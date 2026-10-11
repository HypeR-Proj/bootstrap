// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Guest PCI transport assembled from the claimed function's resource metadata.

use hyper_os::device::{self, FirmwareIdentity, Profile, ProfileInfo, ResourceInfo};
use hyper_os::handle::{
    DeviceAssignmentAuthorityObject, HandleRef, OwnedHandle, PhysicalDeviceObject,
};
use hyper_os::{Error, Result};
use hyper_vm_image::guest_fdt::io::{FirmwareNode, MmioWindow, PciBar, PciHost};

pub struct Assignment {
    device: OwnedHandle<PhysicalDeviceObject>,
    resources: Resources,
}

struct Resources {
    info: ProfileInfo,
    base: u64,
    interrupt_base: u32,
    ecam: MmioWindow,
    msi: MmioWindow,
    bars: Vec<PciBar>,
}

impl Assignment {
    pub fn claim(
        authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
        identity: FirmwareIdentity<'_>,
        base: u64,
        interrupt_base: u32,
    ) -> Result<Self> {
        let FirmwareIdentity::PciId {
            vendor,
            device: device_id,
        } = identity
        else {
            return Err(Error::InvalidResponse);
        };
        let device = device::claim_matching(authority, Profile::PciFunction, identity)?;
        let info = device::profile_info(device.as_handle_ref())?;
        if info.pci_identity != (u32::from(device_id) << 16 | u32::from(vendor)) {
            return Err(Error::InvalidResponse);
        }
        let mut descriptions = Vec::new();
        descriptions
            .try_reserve_exact(info.resource_count as usize)
            .map_err(|_| Error::Status(hyper_os::Status::NO_MEMORY))?;
        for index in 0..info.resource_count {
            descriptions.push(device::resource_info(device.as_handle_ref(), index)?);
        }
        let resources = Resources::project(info, &descriptions, base, interrupt_base)?;
        Ok(Self { device, resources })
    }

    pub fn device(&self) -> &OwnedHandle<PhysicalDeviceObject> {
        &self.device
    }

    pub fn host<'a>(&'a self, nodes: &'a [FirmwareNode<'a>]) -> PciHost<'a> {
        self.resources.host(nodes)
    }

    pub fn bars(&self) -> &[PciBar] {
        &self.resources.bars
    }
}

impl Resources {
    fn project(
        info: ProfileInfo,
        descriptions: &[ResourceInfo],
        base: u64,
        interrupt_base: u32,
    ) -> Result<Self> {
        if info.profile != Profile::PciFunction
            || descriptions.len() != info.resource_count as usize
            || !(1..=64).contains(&info.interrupt_count)
            || interrupt_base
                .checked_add(info.interrupt_count)
                .is_none_or(|end| end > 256)
        {
            return Err(Error::InvalidResponse);
        }
        let mut ecam = None;
        let mut msi = None;
        let mut bars = Vec::new();
        bars.try_reserve_exact(6)
            .map_err(|_| Error::Status(hyper_os::Status::NO_MEMORY))?;
        for resource in descriptions {
            if resource
                .offset
                .checked_add(resource.length)
                .is_none_or(|end| end > info.aperture_size)
            {
                return Err(Error::InvalidResponse);
            }
            let window = MmioWindow {
                base: base
                    .checked_add(resource.offset)
                    .ok_or(Error::InvalidResponse)?,
                size: resource.length,
            };
            window
                .base
                .checked_add(window.size)
                .ok_or(Error::InvalidResponse)?;
            match resource.kind {
                device::RESOURCE_PCI_ECAM if ecam.is_none() => ecam = Some(window),
                device::RESOURCE_PCI_MSI if msi.is_none() => msi = Some(window),
                kind if (device::RESOURCE_PCI_BAR0..device::RESOURCE_PCI_BAR0 + 6)
                    .contains(&kind) =>
                {
                    let index = kind - device::RESOURCE_PCI_BAR0;
                    if bars.iter().any(|bar: &PciBar| bar.index == index) {
                        return Err(Error::InvalidResponse);
                    }
                    bars.push(PciBar {
                        index,
                        window,
                        bus_address: resource.bus_address,
                        memory64: resource.flags & device::RESOURCE_MEMORY_64 != 0,
                        prefetchable: resource.flags & device::RESOURCE_PREFETCHABLE != 0,
                    });
                }
                _ => return Err(Error::InvalidResponse),
            }
        }
        if bars.is_empty() {
            return Err(Error::InvalidResponse);
        }
        Ok(Self {
            info,
            base,
            interrupt_base,
            ecam: ecam.ok_or(Error::InvalidResponse)?,
            msi: msi.ok_or(Error::InvalidResponse)?,
            bars,
        })
    }

    fn host<'a>(&'a self, nodes: &'a [FirmwareNode<'a>]) -> PciHost<'a> {
        PciHost {
            aperture: MmioWindow {
                base: self.base,
                size: self.info.aperture_size,
            },
            ecam: self.ecam,
            msi: self.msi,
            interrupt_base: self.interrupt_base,
            interrupt_count: self.info.interrupt_count,
            bars: &self.bars,
            dma_bus_offset: self.info.dma_bus_offset,
            nodes,
        }
    }
}

#[cfg(test)]
#[path = "../tests/pci.rs"]
mod tests;
