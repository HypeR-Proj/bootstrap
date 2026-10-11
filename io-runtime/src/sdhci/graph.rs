// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::{FirmwareNode as PlatformDevice, MmioResource};

pub(super) struct Graph {
    pub(super) owners: [(u32, u32); 5],
    pub(super) ranges: [MmioResource; 5],
    pub(super) revision: u32,
    pub(super) widths: [u32; 2],
}

fn cells_equal(node: &PlatformDevice, name: &str, expected: &[u32]) -> bool {
    node.property(name).is_some_and(|bytes| {
        bytes.len() == expected.len() * 4
            && bytes
                .chunks_exact(4)
                .zip(expected)
                .all(|(bytes, expected)| bytes == expected.to_be_bytes())
    })
}

fn cell(node: &PlatformDevice, name: &str) -> Option<u32> {
    Some(u32::from_be_bytes(node.property(name)?.try_into().ok()?))
}

fn reference(nodes: &[PlatformDevice], id: u32) -> Option<&PlatformDevice> {
    if id == 0 || id == u32::MAX {
        return None;
    }
    let mut matches = nodes
        .iter()
        .filter(|node| cell(node, "phandle") == Some(id));
    let node = matches.next()?;
    if matches.next().is_some() || node.kernel_claimed() {
        return None;
    }
    Some(node)
}

fn dependency<'a>(
    nodes: &'a [PlatformDevice],
    node: &PlatformDevice,
    name: &str,
) -> Option<&'a PlatformDevice> {
    reference(nodes, cell(node, name)?)
}

fn parent<'a>(nodes: &'a [PlatformDevice], child: &PlatformDevice) -> Option<&'a PlatformDevice> {
    let (path, _) = child.path().rsplit_once('/')?;
    nodes
        .iter()
        .find(|node| node.path() == path && !node.kernel_claimed())
}

pub(super) fn parse(
    device: &PlatformDevice,
    nodes: &[PlatformDevice],
    reserved: impl Fn(u32) -> bool,
) -> Option<Graph> {
    if device.interrupt != Some((305, true))
        || device.kernel_claimed()
        || !device.is_compatible("brcm,bcm2712-sdhci")
        || device.is_compatible("virtio,mmio")
        || device.registers().len() != 2
        || device.property("reg-names") != Some(b"host\0cfg\0")
        || device.property("clock-names") != Some(b"sw_sdio\0")
        || !cells_equal(device, "bus-width", &[4])
        || ["resets", "power-domains", "phys", "iommus", "dma-coherent"]
            .iter()
            .any(|name| device.property(name).is_some())
    {
        return None;
    }
    let clock = dependency(nodes, device, "clocks")?;
    if !clock.is_compatible("fixed-clock")
        || !cells_equal(clock, "#clock-cells", &[0])
        || !cells_equal(clock, "clock-frequency", &[200_000_000])
    {
        return None;
    }
    let voltage = dependency(nodes, device, "vqmmc-supply")?;
    let power = dependency(nodes, device, "vmmc-supply")?;
    if !voltage.is_compatible("regulator-gpio")
        || !power.is_compatible("regulator-fixed")
        || !cells_equal(voltage, "regulator-min-microvolt", &[1_800_000])
        || !cells_equal(voltage, "regulator-max-microvolt", &[3_300_000])
        || !cells_equal(voltage, "regulator-settling-time-us", &[5000])
        || !cells_equal(voltage, "states", &[1_800_000, 1, 3_300_000, 0])
        || !cells_equal(power, "regulator-min-microvolt", &[3_300_000])
        || !cells_equal(power, "regulator-max-microvolt", &[3_300_000])
        || voltage.property("regulator-boot-on").is_none()
        || voltage.property("regulator-always-on").is_none()
        || power.property("regulator-boot-on").is_none()
        || power.property("enable-active-high").is_none()
    {
        return None;
    }
    let gpio_bytes = voltage.property("gpios")?;
    let gpio_id = u32::from_be_bytes(gpio_bytes.get(..4)?.try_into().ok()?);
    let gpio = reference(nodes, gpio_id)?;
    if !cells_equal(voltage, "gpios", &[gpio_id, 3, 0])
        || !cells_equal(power, "gpios", &[gpio_id, 4, 0])
        || !cells_equal(device, "cd-gpios", &[gpio_id, 5, 1])
        || !gpio.is_compatible("brcm,brcmstb-gpio")
        || !cells_equal(gpio, "#gpio-cells", &[2])
        || gpio.property("gpio-controller").is_none()
        || gpio.property("interrupt-controller").is_some()
        || gpio.interrupt.is_some()
        || gpio.property("interrupts").is_some()
        || gpio.property("interrupts-extended").is_some()
    {
        return None;
    }
    let pins = device.property("pinctrl-0")?;
    if pins.len() != 8 || device.property("pinctrl-names") != Some(b"default\0") {
        return None;
    }
    let main_state = reference(nodes, u32::from_be_bytes(pins[..4].try_into().ok()?))?;
    let aon_state = reference(nodes, u32::from_be_bytes(pins[4..].try_into().ok()?))?;
    if main_state.property("pins")
        != Some(b"emmc_cmd\0emmc_dat0\0emmc_dat1\0emmc_dat2\0emmc_dat3\0")
        || main_state.property("bias-pull-up").is_none()
        || main_state.property("function").is_some()
        || aon_state.property("pins") != Some(b"aon_gpio5\0")
        || aon_state.property("function") != Some(b"sd_card_g\0")
        || aon_state.property("bias-pull-up").is_none()
    {
        return None;
    }
    let main = parent(nodes, main_state)?;
    let aon = parent(nodes, aon_state)?;
    let (revision, main_size, aon_size, widths) = if main.is_compatible("brcm,bcm2712c0-pinctrl")
        && aon.is_compatible("brcm,bcm2712c0-aon-pinctrl")
    {
        (0, 0x30, 0x20, [17, 6])
    } else if main.is_compatible("brcm,bcm2712d0-pinctrl")
        && aon.is_compatible("brcm,bcm2712d0-aon-pinctrl")
    {
        (1, 0x20, 0x1c, [15, 6])
    } else {
        return None;
    };
    if !cells_equal(gpio, "brcm,gpio-bank-widths", &widths)
        || [device, clock, voltage, power, main, aon, gpio]
            .iter()
            .any(|node| reserved(node.id()))
        || [main, aon, gpio]
            .iter()
            .any(|node| node.registers().len() != 1)
        || [
            "sd-uhs-sdr50",
            "sd-uhs-ddr50",
            "sd-uhs-sdr104",
            "mmc-ddr-3_3v",
        ]
        .iter()
        .any(|name| device.property(name).is_none())
    {
        return None;
    }
    let ranges = [
        device.registers()[0],
        device.registers()[1],
        main.registers()[0],
        aon.registers()[0],
        gpio.registers()[0],
    ];
    let sizes = [0x260, 0x200, main_size, aon_size, 0x40];
    if ranges
        .iter()
        .zip(sizes)
        .any(|(range, size)| range.size() != size)
        || ranges[0].start().checked_add(0x400) != Some(ranges[1].start())
    {
        return None;
    }
    for (index, range) in ranges.iter().enumerate() {
        if !range.start().is_multiple_of(4) {
            return None;
        }
        let end = range.start().checked_add(range.size())?;
        if range.start() / 4096 != (end - 1) / 4096 {
            return None;
        }
        for (other_index, other) in ranges[..index].iter().enumerate() {
            let other_end = other.start().checked_add(other.size())?;
            if range.start() < other_end && other.start() < end {
                return None;
            }
            if !(index == 1 && other_index == 0) && range.start() / 4096 == other.start() / 4096 {
                return None;
            }
        }
    }
    // Node IDs alone do not exclude a second firmware node aliasing a boot
    // driver's register page. Reserve controller pages against all such owners.
    for owner in nodes
        .iter()
        .filter(|node| node.kernel_claimed() || reserved(node.id()))
    {
        for owned in owner.registers() {
            let owned_end = owned.start().checked_add(owned.size())?.checked_add(4095)? & !4095;
            for range in &ranges {
                let end = range.start().checked_add(range.size())?.checked_add(4095)? & !4095;
                if (owned.start() & !4095) < end && (range.start() & !4095) < owned_end {
                    return None;
                }
            }
        }
    }
    Some(Graph {
        owners: [
            (device.id(), 0),
            (device.id(), 1),
            (main.id(), 0),
            (aon.id(), 0),
            (gpio.id(), 0),
        ],
        ranges,
        revision,
        widths,
    })
}
