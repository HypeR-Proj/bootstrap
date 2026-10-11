// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::{
    cell, cells, descendant, enabled, parent, references,
    tree::{Node, Projection},
};
use crate::firmware::FirmwareNode;
use hyper_os::{Error, Result};
use hyper_vm_image::guest_fdt::io::PciBar;

pub(super) fn project(nodes: &[FirmwareNode], bars: &[PciBar]) -> Result<Projection> {
    let mut clocks = nodes
        .iter()
        .filter(|node| node.is_compatible("raspberrypi,rp1-clocks") && available(nodes, node));
    let clock = clocks.next().ok_or(Error::InvalidResponse)?;
    if clocks.next().is_some() {
        return Err(Error::InvalidResponse);
    }
    let bus_path = parent(clock.path());
    let bus = nodes
        .iter()
        .find(|node| node.path() == bus_path)
        .ok_or(Error::InvalidResponse)?;
    let native_nexus = nodes
        .iter()
        .find(|node| node.path() == parent(bus_path) && node.is_compatible("pci1de4,1"));
    let source_root = native_nexus.unwrap_or(bus);
    let mut selected: Vec<bool> = nodes
        .iter()
        .map(|node| {
            node.path() == source_root.path() || descendant(node.path(), source_root.path())
        })
        .collect();
    let mut visited = vec![false; nodes.len()];
    while let Some(index) = selected
        .iter()
        .enumerate()
        .position(|(index, selected)| *selected && !visited[index])
    {
        visited[index] = true;
        let node = &nodes[index];
        // Disabled firmware remains disabled and opaque. Dependencies of an
        // enabled device must be fully available; no host driver is imported.
        if !available(nodes, node) || node.path() == source_root.path() {
            continue;
        }
        for target in references::referenced(nodes, node)? {
            let dependency = &nodes[target];
            if !available(nodes, dependency) {
                return Err(Error::InvalidResponse);
            }
            if !selected[target] {
                validate_external(dependency)?;
                selected[target] = true;
                for (child, include) in nodes.iter().zip(&mut selected) {
                    if descendant(child.path(), dependency.path()) {
                        if available(nodes, child) {
                            validate_external(child)?;
                        }
                        *include = true;
                    }
                }
            }
        }
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(selected.iter().filter(|value| **value).count() + 2)
        .map_err(|_| Error::Status(hyper_os::Status::NO_MEMORY))?;
    for (_, source) in nodes
        .iter()
        .enumerate()
        .filter(|(index, _)| selected[*index])
    {
        if source.path() == source_root.path() {
            continue;
        }
        let path = if let Some(relative) = source
            .path()
            .strip_prefix(source_root.path())
            .filter(|path| path.starts_with('/'))
        {
            if native_nexus.is_some() {
                format!("rp1_nexus{relative}")
            } else {
                format!("rp1_nexus/pci-ep-bus@1{relative}")
            }
        } else {
            // Flatten only dependency roots; retain their child hierarchy.
            let ancestor = nodes
                .iter()
                .enumerate()
                .filter(|(index, candidate)| {
                    selected[*index]
                        && !descendant(candidate.path(), source_root.path())
                        && descendant(source.path(), candidate.path())
                })
                .min_by_key(|(_, candidate)| candidate.path().len())
                .map(|(_, node)| node);
            let root = ancestor.unwrap_or(source);
            let name = root
                .path()
                .rsplit('/')
                .next()
                .ok_or(Error::InvalidResponse)?;
            let relative = source
                .path()
                .strip_prefix(root.path())
                .ok_or(Error::InvalidResponse)?;
            format!("rp1_nexus/pci-ep-bus@1/firmware-dependencies/{name}{relative}")
        };
        let mut node = Node {
            path,
            properties: source.properties.clone(),
        };
        if node.properties.iter().all(|(name, _)| name != "compatible")
            && !source.compatible.is_empty()
        {
            node.set(
                "compatible",
                &source
                    .compatible
                    .iter()
                    .flat_map(|value| value.bytes().chain([0]))
                    .collect::<Vec<_>>(),
            );
        }
        // The original DMA apertures describe host RAM. Empty intermediate
        // ranges make Linux obtain the exact admitted guest RAM translations
        // from the synthetic PCI host, rather than translating an old broad
        // aperture's base which may not belong to this VM at all.
        if node
            .properties
            .iter()
            .any(|(name, _)| matches!(name.as_str(), "dma-ranges" | "#address-cells"))
            || nodes
                .iter()
                .any(|child| parent(child.path()) == source.path())
        {
            node.set("dma-ranges", &[]);
        }
        output.push(node);
    }
    let mut nexus = Node {
        path: "rp1_nexus".into(),
        properties: Vec::new(),
    };
    nexus.set("compatible", b"pci1de4,1\0");
    nexus.set_cells("reg", &[0, 0, 0, 0, 0]);
    nexus.set_cells("#address-cells", &[3]);
    nexus.set_cells("#size-cells", &[2]);
    nexus.set_cells("#interrupt-cells", &[2]);
    nexus.set("interrupt-controller", &[]);
    nexus.set_cells("phandle", &[cell(source_root, "phandle")?]);
    nexus.set("dma-ranges", &[]);
    let mut ranges = Vec::new();
    for bar in bars {
        let flags = if bar.memory64 {
            0x0300_0000
        } else {
            0x0200_0000
        } | if bar.prefetchable { 0x4000_0000 } else { 0 };
        ranges.extend([
            bar.index,
            0,
            0,
            flags,
            (bar.bus_address >> 32) as u32,
            bar.bus_address as u32,
            (bar.window.size >> 32) as u32,
            bar.window.size as u32,
        ]);
    }
    nexus.set_cells("ranges", &ranges);
    output.push(nexus);
    if native_nexus.is_none() {
        output.push(legacy_bus(bus, bars)?);
    }
    if output.iter().any(|node| {
        node.path
            .starts_with("rp1_nexus/pci-ep-bus@1/firmware-dependencies/")
    }) {
        let mut dependencies = Node {
            path: "rp1_nexus/pci-ep-bus@1/firmware-dependencies".into(),
            properties: Vec::new(),
        };
        dependencies.set("compatible", b"simple-bus\0");
        dependencies.set_cells("#address-cells", &[2]);
        dependencies.set_cells("#size-cells", &[2]);
        dependencies.set("ranges", &[]);
        dependencies.set("dma-ranges", &[]);
        output.push(dependencies);
    }
    super::normalize::phy_reset(&mut output)?;
    Projection::new(output)
}

fn available(nodes: &[FirmwareNode], node: &FirmwareNode) -> bool {
    enabled(node)
        && !nodes
            .iter()
            .any(|ancestor| descendant(node.path(), ancestor.path()) && !enabled(ancestor))
}

fn validate_external(node: &FirmwareNode) -> Result<()> {
    if node.kernel_claimed()
        || !node.registers().is_empty()
        || node.property("reg").is_some()
        || node.property("interrupts").is_some()
        || node.property("interrupts-extended").is_some()
        || node.property("interrupt-controller").is_some()
        || node.property("iommus").is_some()
        || !node.compatible.iter().any(|name| {
            matches!(
                name.as_str(),
                "fixed-clock"
                    | "fixed-factor-clock"
                    | "regulator-fixed"
                    | "raspberrypi,rp1-firmware"
            )
        })
    {
        return Err(Error::InvalidResponse);
    }
    Ok(())
}

fn legacy_bus(source: &FirmwareNode, bars: &[PciBar]) -> Result<Node> {
    // Raspberry Pi firmware describes APB and SRAM as one contiguous simple
    // bus. PCI exposes the same ranges as BAR1 and BAR2. Their sizes remain
    // authoritative capability metadata, while the child address is firmware.
    if cell(source, "#address-cells")? != 2 || cell(source, "#size-cells")? != 2 {
        return Err(Error::InvalidResponse);
    }
    let ranges = cells(source.property("ranges").ok_or(Error::InvalidResponse)?)?;
    if ranges.len() != 7
        || ranges[2] & 0x0300_0000 != 0x0200_0000
        || ranges[3] != 0
        || ranges[4] != 0
    {
        return Err(Error::InvalidResponse);
    }
    let base = (u64::from(ranges[0]) << 32) | u64::from(ranges[1]);
    let length = (u64::from(ranges[5]) << 32) | u64::from(ranges[6]);
    let apb = bars
        .iter()
        .find(|bar| bar.index == 1)
        .ok_or(Error::InvalidResponse)?;
    let sram = bars
        .iter()
        .find(|bar| bar.index == 2)
        .ok_or(Error::InvalidResponse)?;
    if apb.window.size.checked_add(sram.window.size) != Some(length) {
        return Err(Error::InvalidResponse);
    }
    let sram_base = base
        .checked_add(apb.window.size)
        .ok_or(Error::InvalidResponse)?;
    let mut node = Node {
        path: "rp1_nexus/pci-ep-bus@1".into(),
        properties: Vec::new(),
    };
    node.set("compatible", b"simple-bus\0");
    node.set_cells("#address-cells", &[2]);
    node.set_cells("#size-cells", &[2]);
    node.set_cells("interrupt-parent", &[cell(source, "phandle")?]);
    node.set_cells(
        "ranges",
        &[
            (base >> 32) as u32,
            base as u32,
            1,
            0,
            0,
            (apb.window.size >> 32) as u32,
            apb.window.size as u32,
            (sram_base >> 32) as u32,
            sram_base as u32,
            2,
            0,
            0,
            (sram.window.size >> 32) as u32,
            sram.window.size as u32,
        ],
    );
    // Let Linux discover the PCI host's exact admitted DMA ranges above.
    node.set("dma-ranges", &[]);
    Ok(node)
}
