// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

// Plain firmware facts: no kernel parser or source inclusion in app tests.
struct Tree {
    nodes: Vec<FirmwareNode>,
    stack: Vec<usize>,
}
impl Tree {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            stack: Vec::new(),
        }
    }
    fn begin(&mut self, name: &str) {
        let parent = self
            .stack
            .last()
            .map(|&index| self.nodes[index].path.as_str())
            .unwrap_or("");
        let path = format!("{parent}/{name}");
        let path = if path.starts_with("//") {
            path[1..].to_string()
        } else {
            path
        };
        let id = self.nodes.len() as u32;
        self.nodes.push(FirmwareNode {
            id,
            path,
            compatible: Vec::new(),
            registers: Vec::new(),
            properties: Vec::new(),
            kernel_owned: false,
            interrupt: None,
        });
        self.stack.push(id as usize);
    }
    fn end(&mut self) {
        self.stack.pop();
    }
    fn prop(&mut self, name: &str, value: &[u8]) {
        let node = &mut self.nodes[*self.stack.last().unwrap()];
        if name == "compatible" {
            node.compatible = core::str::from_utf8(value)
                .unwrap()
                .trim_end_matches('\0')
                .split('\0')
                .map(String::from)
                .collect();
        } else if name == "reg" {
            node.registers = value
                .chunks_exact(16)
                .map(|entry| MmioResource {
                    base: u64::from_be_bytes(entry[..8].try_into().unwrap()),
                    length: u64::from_be_bytes(entry[8..].try_into().unwrap()),
                })
                .collect();
        } else {
            node.properties.push((name.to_string(), value.to_vec()));
        }
    }
    fn cells(&mut self, name: &str, values: &[u32]) {
        self.prop(
            name,
            &values
                .iter()
                .flat_map(|value| value.to_be_bytes())
                .collect::<Vec<_>>(),
        );
    }
    fn flag(&mut self, name: &str) {
        self.prop(name, &[]);
    }
    fn text(&mut self, name: &str, value: &str) {
        self.prop(name, format!("{value}\0").as_bytes());
    }
    fn reg(&mut self, base: u64, size: u64) {
        self.cells(
            "reg",
            &[
                (base >> 32) as u32,
                base as u32,
                (size >> 32) as u32,
                size as u32,
            ],
        );
    }
    fn finish(mut self) -> Vec<FirmwareNode> {
        self.nodes
            .iter_mut()
            .find(|node| node.path == "/sd")
            .unwrap()
            .interrupt = Some((305, true));
        self.nodes
    }
}
fn fixture(d0: bool, defect: &str) -> Vec<FirmwareNode> {
    let mut tree = Tree::new();
    tree.begin("");
    tree.cells("#address-cells", &[2]);
    tree.cells("#size-cells", &[2]);
    tree.begin("memory@40000000");
    tree.text("device_type", "memory");
    tree.reg(0x40000000, 0x10000000);
    tree.end();
    tree.begin("clock");
    tree.text("compatible", "fixed-clock");
    tree.cells("phandle", &[10]);
    tree.cells("#clock-cells", &[0]);
    tree.cells(
        "clock-frequency",
        &[if defect == "clock" { 100 } else { 200_000_000 }],
    );
    tree.end();
    tree.begin("voltage");
    tree.text("compatible", "regulator-gpio");
    tree.cells("phandle", &[11]);
    tree.cells("regulator-min-microvolt", &[1_800_000]);
    tree.cells("regulator-max-microvolt", &[3_300_000]);
    tree.flag("regulator-boot-on");
    tree.flag("regulator-always-on");
    tree.cells("regulator-settling-time-us", &[5000]);
    tree.cells("gpios", &[13, 3, 0]);
    tree.cells("states", &[1_800_000, 1, 3_300_000, 0]);
    tree.end();
    tree.begin("power");
    tree.text("compatible", "regulator-fixed");
    tree.cells("phandle", &[12]);
    tree.cells("regulator-min-microvolt", &[3_300_000]);
    tree.cells("regulator-max-microvolt", &[3_300_000]);
    tree.flag("regulator-boot-on");
    tree.flag("enable-active-high");
    tree.cells("gpios", &[13, if defect == "gpio" { 6 } else { 4 }, 0]);
    tree.end();
    tree.begin("gpio");
    tree.text("compatible", "brcm,brcmstb-gpio");
    tree.cells("phandle", &[13]);
    tree.cells("#gpio-cells", &[2]);
    tree.flag("gpio-controller");
    tree.cells("brcm,gpio-bank-widths", &[if d0 { 15 } else { 17 }, 6]);
    tree.reg(0x107d517c00, 0x40);
    if defect == "irq" {
        tree.flag("interrupt-controller");
    }
    tree.end();
    tree.begin("main");
    tree.text(
        "compatible",
        if d0 {
            "brcm,bcm2712d0-pinctrl"
        } else {
            "brcm,bcm2712c0-pinctrl"
        },
    );
    tree.reg(
        if defect == "overlap" {
            0x107d517c00
        } else {
            0x107d504100
        },
        if d0 { 0x20 } else { 0x30 },
    );
    tree.begin("pins");
    tree.cells("phandle", &[14]);
    tree.prop(
        "pins",
        b"emmc_cmd\0emmc_dat0\0emmc_dat1\0emmc_dat2\0emmc_dat3\0",
    );
    tree.flag("bias-pull-up");
    tree.end();
    tree.end();
    tree.begin("aon");
    tree.text(
        "compatible",
        if d0 {
            "brcm,bcm2712d0-aon-pinctrl"
        } else {
            "brcm,bcm2712c0-aon-pinctrl"
        },
    );
    tree.reg(0x107d510700, if d0 { 0x1c } else { 0x20 });
    tree.begin("pins");
    tree.cells("phandle", &[if defect == "duplicate" { 14 } else { 15 }]);
    tree.text("pins", "aon_gpio5");
    tree.text("function", "sd_card_g");
    tree.flag("bias-pull-up");
    tree.end();
    tree.end();
    tree.begin("firmware-owner");
    tree.text("compatible", "test,boot-device");
    tree.reg(0x107d517000, 0x30);
    tree.end();
    tree.begin("sd");
    tree.text("compatible", "brcm,bcm2712-sdhci");
    tree.cells(
        "reg",
        &[0x10, 0x00fff000, 0, 0x260, 0x10, 0x00fff400, 0, 0x200],
    );
    tree.prop("reg-names", b"host\0cfg\0");
    tree.text("clock-names", "sw_sdio");
    tree.cells("clocks", &[10]);
    tree.cells("bus-width", &[4]);
    tree.cells("vqmmc-supply", &[11]);
    tree.cells("vmmc-supply", &[12]);
    tree.cells("cd-gpios", &[13, 5, 1]);
    tree.cells("pinctrl-0", &[14, 15]);
    tree.text("pinctrl-names", "default");
    for flag in [
        "sd-uhs-sdr50",
        "sd-uhs-ddr50",
        "sd-uhs-sdr104",
        "mmc-ddr-3_3v",
    ] {
        tree.flag(flag);
    }
    if defect == "coherent" {
        tree.flag("dma-coherent");
    }
    tree.end();
    tree.end();
    tree.finish()
}

fn require_some<T>(value: Option<T>) -> T {
    value.unwrap()
}
#[test]
fn upstream_sdio_dependencies_are_validated_before_hardware_access() {
    for d0 in [false, true] {
        let devices = fixture(d0, "");
        let sd = require_some(devices.iter().find(|node| node.path() == "/sd"));
        let graph = require_some(graph::parse(sd, &devices, |_| false));
        assert_eq!(graph.revision, u32::from(d0));
        assert_eq!(graph.widths, [if d0 { 15 } else { 17 }, 6]);
        assert_eq!(graph.ranges[0].start(), 0x1000fff000);
        assert_eq!(graph.ranges[4].size(), 0x40);
        assert!(graph::parse(sd, &devices, |_| true).is_none());
    }
    for defect in ["clock", "gpio", "irq", "overlap", "duplicate", "coherent"] {
        let devices = fixture(false, defect);
        let sd = require_some(devices.iter().find(|node| node.path() == "/sd"));
        assert!(graph::parse(sd, &devices, |_| false).is_none(), "{defect}");
    }
}

#[test]
fn dependency_snapshot_marks_boot_owned_controllers() {
    let mut devices = fixture(false, "");
    devices
        .iter_mut()
        .find(|node| node.path == "/gpio")
        .unwrap()
        .kernel_owned = true;
    let sd = devices.iter().find(|node| node.path == "/sd").unwrap();
    assert!(graph::parse(sd, &devices, |_| false).is_none());
}

#[test]
fn essential_device_alias_cannot_be_hidden_behind_another_node() {
    let devices = fixture(false, "");
    let sd = require_some(devices.iter().find(|node| node.path() == "/sd"));
    let owner = require_some(devices.iter().find(|node| node.path() == "/firmware-owner")).id();
    assert!(graph::parse(sd, &devices, |id| id == owner).is_none());
}

#[test]
fn plan_uses_firmware_resource_indices_and_owned_guest_offsets() {
    for d0 in [false, true] {
        let devices = fixture(d0, "");
        let plan = plan(&devices, FirmwareIdentity::FdtPath("/sd")).unwrap();
        assert_eq!(plan.profile.revision, u32::from(d0));
        assert_eq!(plan.profile.clock_hz, 200_000_000);
        for (index, (path, resource, offset)) in [
            ("/sd", 0, 0),
            ("/sd", 1, 0x400),
            ("/main", 0, 0x1100),
            ("/aon", 0, 0x2700),
            ("/gpio", 0, 0x3c00),
        ]
        .into_iter()
        .enumerate()
        {
            let node = devices.iter().find(|node| node.path == path).unwrap();
            assert_eq!(plan.entries[index].node, node.id);
            assert_eq!(plan.entries[index].resource, resource);
            assert_eq!(plan.entries[index].offset, offset);
            assert_eq!(plan.profile.resources[index].offset, offset);
            assert_eq!(
                plan.profile.resources[index].length,
                node.registers[resource as usize].length
            );
        }
        assert_eq!(
            plan.irq_node,
            devices.iter().find(|node| node.path == "/sd").unwrap().id
        );
    }
}

#[test]
fn selector_never_skips_an_invalid_or_owned_match() {
    let mut devices = fixture(false, "");
    let mut duplicate = devices
        .iter()
        .find(|node| node.path == "/sd")
        .unwrap()
        .clone();
    duplicate.path = "/wifi".into();
    duplicate.id = devices.len() as u32;
    duplicate.kernel_owned = true;
    for register in &mut duplicate.registers {
        register.base += 0x10000;
    }
    devices.push(duplicate);
    assert!(plan(&devices, FirmwareIdentity::Compatible("brcm,bcm2712-sdhci")).is_err());
    assert!(plan(&devices, FirmwareIdentity::FdtPath("/sd")).is_ok());
    assert!(plan(&devices, FirmwareIdentity::FdtPath("/wifi")).is_err());
    assert!(plan(&devices, FirmwareIdentity::FdtPath("/missing")).is_err());
}

#[test]
fn unexpected_interrupts_and_register_geometry_are_rejected() {
    for interrupt in [None, Some((305, false)), Some((306, true))] {
        let mut devices = fixture(false, "");
        let sd = devices.iter_mut().find(|node| node.path == "/sd").unwrap();
        sd.interrupt = interrupt;
        assert!(plan(&devices, FirmwareIdentity::FdtPath("/sd")).is_err());
    }
    for (base, length) in [
        (0x107d504101, 0x30),
        (0x107d504ff0, 0x30),
        (u64::MAX - 15, 0x30),
        (0x107d504100, 0),
    ] {
        let mut devices = fixture(false, "");
        devices
            .iter_mut()
            .find(|node| node.path == "/main")
            .unwrap()
            .registers[0] = MmioResource { base, length };
        assert!(plan(&devices, FirmwareIdentity::FdtPath("/sd")).is_err());
    }
}

#[test]
fn raw_gpio_interrupt_dependencies_cannot_hide_behind_decode_failure() {
    for name in ["interrupts", "interrupts-extended"] {
        let mut nodes = fixture(false, "");
        nodes
            .iter_mut()
            .find(|node| node.path == "/gpio")
            .unwrap()
            .properties
            .push((name.into(), vec![0xff]));
        assert!(plan(&nodes, FirmwareIdentity::FdtPath("/sd")).is_err());
    }
}

#[cfg(feature = "userspace-device-test")]
#[test]
fn virtio_test_claim_requires_exact_unowned_level_firmware_node() {
    let mut node = FirmwareNode {
        id: 7,
        path: "/virtio_mmio@a000000".into(),
        compatible: vec!["virtio,mmio".into()],
        registers: vec![MmioResource {
            base: 0x0a000000,
            length: 0x200,
        }],
        properties: vec![],
        kernel_owned: false,
        interrupt: Some((48, true)),
    };
    let id = FirmwareIdentity::FdtPath("/virtio_mmio@a000000");
    assert_eq!(virtio_test_node(&[node.clone()], id), Ok(7));
    assert!(
        virtio_test_node(&[node.clone()], FirmwareIdentity::Compatible("virtio,mmio")).is_err()
    );
    assert!(virtio_test_node(&[node.clone(), node.clone()], id).is_err());
    node.interrupt = Some((48, false));
    assert!(virtio_test_node(&[node.clone()], id).is_err());
    node.interrupt = Some((48, true));
    node.kernel_owned = true;
    assert!(virtio_test_node(&[node.clone()], id).is_err());
    node.kernel_owned = false;
    node.registers[0].length = 0x80;
    assert!(virtio_test_node(&[node], id).is_err());
}
