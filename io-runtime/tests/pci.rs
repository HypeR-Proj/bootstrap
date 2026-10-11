// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn info() -> ProfileInfo {
    ProfileInfo {
        profile: Profile::PciFunction,
        interrupt_count: 61,
        resource_count: 5,
        pci_identity: 0x0001_1de4,
        dma_bus_offset: 0x10_0000_0000,
        aperture_size: 0x80_0000,
    }
}
fn resources() -> Vec<ResourceInfo> {
    [
        (device::RESOURCE_PCI_ECAM, 0, 0x10_0000),
        (device::RESOURCE_PCI_MSI, 0x10_0000, 4096),
        (device::RESOURCE_PCI_BAR0, 0x20_0000, 0x4000),
        (device::RESOURCE_PCI_BAR0 + 1, 0x40_0000, 0x40_0000),
        (device::RESOURCE_PCI_BAR0 + 2, 0x21_0000, 0x1_0000),
    ]
    .into_iter()
    .map(|(kind, offset, length)| ResourceInfo {
        kind,
        flags: 0,
        offset,
        length,
        bus_address: if kind >= device::RESOURCE_PCI_BAR0 {
            offset
        } else {
            0
        },
    })
    .collect()
}

#[test]
fn full_function_transport_uses_admitted_bar_and_vector_metadata() {
    let projected = Resources::project(info(), &resources(), 0xb80_0000, 128).unwrap();
    let host = projected.host(&[]);
    assert_eq!(host.interrupt_count, 61);
    assert_eq!(
        host.ecam,
        MmioWindow {
            base: 0xb80_0000,
            size: 0x10_0000
        }
    );
    assert_eq!(host.msi.base, 0xb90_0000);
    assert_eq!(host.bars[1].bus_address, 0x40_0000);
    assert_eq!(host.bars[1].window.base, 0xbc0_0000);
    assert_eq!(host.dma_bus_offset, 0x10_0000_0000);
}

#[test]
fn missing_duplicate_or_outside_resources_never_produce_partial_host() {
    let mut records = resources();
    records[1] = records[0];
    assert!(Resources::project(info(), &records, 0xb80_0000, 128).is_err());
    records = resources();
    records[4] = records[3];
    assert!(Resources::project(info(), &records, 0xb80_0000, 128).is_err());
    records = resources();
    records[4].offset = 0x80_0000;
    assert!(Resources::project(info(), &records, 0xb80_0000, 128).is_err());
    assert!(Resources::project(info(), &resources()[..4], 0xb80_0000, 128).is_err());
    assert!(Resources::project(info(), &resources(), 0xb80_0000, 250).is_err());
}
