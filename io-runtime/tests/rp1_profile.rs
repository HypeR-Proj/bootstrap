// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::firmware::{FirmwareNode, MmioResource};

fn node(path: &str, compatible: &str, handle: u32) -> FirmwareNode {
    let mut node = FirmwareNode {
        id: handle,
        path: path.into(),
        compatible: vec![compatible.into()],
        registers: Vec::new(),
        properties: Vec::new(),
        kernel_owned: false,
        interrupt: None,
    };
    set_cells(&mut node, "phandle", &[handle]);
    node
}
fn set(node: &mut FirmwareNode, name: &str, value: &[u8]) {
    node.properties.retain(|(key, _)| key != name);
    node.properties.push((name.into(), value.into()));
}
fn set_cells(node: &mut FirmwareNode, name: &str, values: &[u32]) {
    set(
        node,
        name,
        &values
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect::<Vec<_>>(),
    );
}
fn firmware() -> Vec<FirmwareNode> {
    let mut root = node("/pcie/rp1", "simple-bus", 40);
    set_cells(&mut root, "#address-cells", &[2]);
    set_cells(&mut root, "#size-cells", &[2]);
    set_cells(
        &mut root,
        "ranges",
        &[0xc0, 0x40000000, 0x02000000, 0, 0, 0, 0x410000],
    );
    let mut clock = node("/pcie/rp1/clocks@18000", "raspberrypi,rp1-clocks", 2);
    set_cells(&mut clock, "clocks", &[41]);
    set_cells(&mut clock, "#clock-cells", &[1]);
    set_cells(&mut clock, "reg", &[0xc0, 0x40018000, 0, 0x10038]);
    let mut xosc = node("/clocks/xosc", "fixed-clock", 41);
    set_cells(&mut xosc, "#clock-cells", &[0]);
    set_cells(&mut xosc, "clock-frequency", &[50_000_000]);
    let mut gpio = node("/pcie/rp1/gpio@d0000", "raspberrypi,rp1-gpio", 46);
    set_cells(&mut gpio, "#gpio-cells", &[2]);
    set(&mut gpio, "gpio-controller", &[]);
    let mut gem = node("/pcie/rp1/ethernet@100000", "raspberrypi,rp1-gem", 50);
    set_cells(&mut gem, "clocks", &[2, 12, 2, 12, 2, 29, 2, 16]);
    set_cells(&mut gem, "phy-handle", &[51]);
    set_cells(&mut gem, "phy-reset-gpios", &[46, 32, 1]);
    set_cells(&mut gem, "phy-reset-duration", &[5]);
    set(&mut gem, "local-mac-address", &[2, 3, 4, 5, 6, 7]);
    set(&mut gem, "phy-mode", b"rgmii-id\0");
    let mut phy = node("/pcie/rp1/ethernet@100000/ethernet-phy@1", "", 51);
    set_cells(&mut phy, "reg", &[1]);
    let mut usb = node("/pcie/rp1/usb@200000", "snps,dwc3", 52);
    set(&mut usb, "status", b"okay\0");
    set(&mut usb, "dr_mode", b"host\0");
    set(&mut usb, "snps,parkmode-disable-ss-quirk", &[]);
    let mut disabled = node("/pcie/rp1/csi@110000", "raspberrypi,rp1-cfe", 53);
    set(&mut disabled, "status", b"disabled\0");
    set_cells(&mut disabled, "iommus", &[99]);
    vec![root, clock, xosc, gpio, gem, phy, usb, disabled]
}
fn bars() -> Vec<PciBar> {
    [
        (0, 0x20_0000, 0x4000),
        (1, 0x40_0000, 0x40_0000),
        (2, 0x21_0000, 0x1_0000),
    ]
    .into_iter()
    .map(|(index, offset, size)| PciBar {
        index,
        window: hyper_vm_image::guest_fdt::io::MmioWindow {
            base: 0xb80_0000 + offset,
            size,
        },
        bus_address: offset,
        memory64: false,
        prefetchable: false,
    })
    .collect()
}
fn find<'a>(
    tree: &'a [hyper_vm_image::guest_fdt::io::FirmwareNode<'a>],
    name: &str,
) -> Option<&'a hyper_vm_image::guest_fdt::io::FirmwareNode<'a>> {
    for node in tree {
        if node.name == name {
            return Some(node);
        }
        if let Some(found) = find(node.children, name) {
            return Some(found);
        }
    }
    None
}
fn property<'a>(node: &'a hyper_vm_image::guest_fdt::io::FirmwareNode<'a>, name: &str) -> &'a [u8] {
    node.properties
        .iter()
        .find(|property| property.name == name)
        .unwrap()
        .value
}

#[test]
fn whole_function_preserves_enabled_and_disabled_peripherals_and_references() {
    let projection = graph::project(&firmware(), &bars()).unwrap();
    projection.with_nodes(|tree| {
        let nexus = find(tree, "rp1_nexus").unwrap();
        assert_eq!(property(nexus, "phandle"), 40u32.to_be_bytes());
        let usb = find(tree, "usb@200000").unwrap();
        assert_eq!(property(usb, "status"), b"okay\0");
        assert!(property(usb, "snps,parkmode-disable-ss-quirk").is_empty());
        let gem = find(tree, "ethernet@100000").unwrap();
        assert_eq!(property(gem, "phy-handle"), 51u32.to_be_bytes());
        assert_eq!(property(gem, "local-mac-address"), [2, 3, 4, 5, 6, 7]);
        assert_eq!(
            property(find(tree, "csi@110000").unwrap(), "status"),
            b"disabled\0"
        );
        assert_eq!(
            property(find(tree, "xosc").unwrap(), "clock-frequency"),
            50_000_000u32.to_be_bytes()
        );
        let bus = find(tree, "pci-ep-bus@1").unwrap();
        let ranges = cells(property(bus, "ranges")).unwrap();
        assert_eq!(
            ranges,
            [
                0xc0, 0x40000000, 1, 0, 0, 0, 0x400000, 0xc0, 0x40400000, 2, 0, 0, 0, 0x10000
            ]
        );
    });
}

#[test]
fn missing_or_external_hardware_dependencies_fail_without_importing_host_authority() {
    let mut nodes = firmware();
    nodes.remove(2);
    assert!(graph::project(&nodes, &bars()).is_err());
    let mut nodes = firmware();
    nodes[2].registers.push(MmioResource {
        base: 0x12340000,
        length: 4096,
    });
    assert!(graph::project(&nodes, &bars()).is_err());
    let mut nodes = firmware();
    set(&mut nodes[2], "status", b"disabled\0");
    assert!(graph::project(&nodes, &bars()).is_err());
    let mut nodes = firmware();
    set(&mut nodes[7], "status", b"okay\0");
    assert!(graph::project(&nodes, &bars()).is_err());
    let mut nodes = firmware();
    nodes.push(nodes[2].clone());
    assert!(graph::project(&nodes, &bars()).is_err());
    let mut nodes = firmware();
    set_cells(&mut nodes[4], "clocks", &[2]);
    assert!(graph::project(&nodes, &bars()).is_err());
}

#[test]
fn projection_rejects_bar_mismatch_and_keeps_board_values_instead_of_fixed_defaults() {
    let mut wrong = bars();
    wrong[2].window.size = 0x20000;
    assert!(graph::project(&firmware(), &wrong).is_err());
    let mut nodes = firmware();
    set_cells(&mut nodes[2], "clock-frequency", &[25_000_000]);
    set_cells(&mut nodes[4], "phy-reset-gpios", &[46, 31, 0]);
    graph::project(&nodes, &bars()).unwrap().with_nodes(|tree| {
        assert_eq!(
            property(find(tree, "xosc").unwrap(), "clock-frequency"),
            25_000_000u32.to_be_bytes()
        );
        assert_eq!(
            cells(property(find(tree, "mdio").unwrap(), "reset-gpios")).unwrap(),
            [46, 31, 0]
        );
    });
}

#[test]
fn tree_views_keep_prefix_siblings_and_dma_uses_only_admitted_host_ranges() {
    let mut nodes = firmware();
    for (path, handle) in [
        ("/pcie/rp1/foo", 60),
        ("/pcie/rp1/foo-bar", 61),
        ("/pcie/rp1/foo/child", 62),
        ("/pcie/rp1/foo-bar/child", 63),
    ] {
        nodes.push(node(path, "simple-bus", handle));
    }
    let projection = graph::project(&nodes, &bars()).unwrap();
    projection.with_nodes(|tree| {
        let foo = find(tree, "foo").unwrap();
        let dashed = find(tree, "foo-bar").unwrap();
        assert_eq!(foo.children.len(), 1);
        assert_eq!(dashed.children.len(), 1);
        assert_eq!(property(&foo.children[0], "phandle"), 62u32.to_be_bytes());
        assert_eq!(
            property(&dashed.children[0], "phandle"),
            63u32.to_be_bytes()
        );
        assert!(property(find(tree, "pci-ep-bus@1").unwrap(), "dma-ranges").is_empty());
        assert!(property(find(tree, "rp1_nexus").unwrap(), "dma-ranges").is_empty());
    });
}

#[test]
fn upstream_bar_indexed_nexus_retains_devices_and_external_clock_dependencies() {
    let mut nodes = firmware();
    for node in &mut nodes {
        if node.path == "/pcie/rp1" {
            node.path = "/pcie/rp1_nexus".into();
            node.compatible = vec!["pci1de4,1".into()];
            set_cells(node, "#address-cells", &[3]);
        } else if let Some(relative) = node.path.strip_prefix("/pcie/rp1/") {
            node.path = format!("/pcie/rp1_nexus/pci-ep-bus@1/{relative}");
        }
    }
    let mut bus = node("/pcie/rp1_nexus/pci-ep-bus@1", "simple-bus", 64);
    set_cells(&mut bus, "#address-cells", &[2]);
    set_cells(&mut bus, "#size-cells", &[2]);
    set_cells(&mut bus, "interrupt-parent", &[40]);
    let original_ranges = [
        0xc0, 0x40000000, 1, 0, 0, 0, 0x400000, 0xc0, 0x40400000, 2, 0, 0, 0, 0x10000,
    ];
    set_cells(&mut bus, "ranges", &original_ranges);
    set_cells(
        &mut bus,
        "dma-ranges",
        &[0x10, 0, 0x43000000, 0x10, 0, 0x10, 0],
    );
    nodes.push(bus);
    graph::project(&nodes, &bars()).unwrap().with_nodes(|tree| {
        let nexus = find(tree, "rp1_nexus").unwrap();
        assert_eq!(property(nexus, "phandle"), 40u32.to_be_bytes());
        let ranges = cells(property(nexus, "ranges")).unwrap();
        let apb = ranges.chunks_exact(8).find(|range| range[0] == 1).unwrap();
        assert_eq!(apb, [1, 0, 0, 0x02000000, 0, 0x400000, 0, 0x400000]);
        let bus = find(tree, "pci-ep-bus@1").unwrap();
        assert_eq!(cells(property(bus, "ranges")).unwrap(), original_ranges);
        assert!(property(bus, "dma-ranges").is_empty());
        assert_eq!(property(bus, "interrupt-parent"), 40u32.to_be_bytes());
        assert_eq!(
            property(find(tree, "usb@200000").unwrap(), "status"),
            b"okay\0"
        );
        assert_eq!(
            property(find(tree, "xosc").unwrap(), "phandle"),
            41u32.to_be_bytes()
        );
        assert_eq!(
            property(find(tree, "clocks@18000").unwrap(), "clocks"),
            41u32.to_be_bytes()
        );
    });
}

#[test]
fn legacy_phy_reset_becomes_upstream_mdio_binding_without_changing_identity() {
    let mut nodes = firmware();
    set_cells(&mut nodes[4], "phy-reset-duration", &[7]);
    graph::project(&nodes, &bars()).unwrap().with_nodes(|tree| {
        let mac = find(tree, "ethernet@100000").unwrap();
        assert!(
            !mac.properties
                .iter()
                .any(|property| property.name.starts_with("phy-reset-"))
        );
        let mdio = mac
            .children
            .iter()
            .find(|node| node.name == "mdio")
            .unwrap();
        assert_eq!(property(mdio, "reset-delay-us"), 7000u32.to_be_bytes());
        assert_eq!(cells(property(mdio, "reset-gpios")).unwrap(), [46, 32, 1]);
        let phy = mdio
            .children
            .iter()
            .find(|node| node.name == "ethernet-phy@1")
            .unwrap();
        assert_eq!(property(phy, "phandle"), 51u32.to_be_bytes());
        assert_eq!(property(mac, "phy-handle"), property(phy, "phandle"));
    });
    set_cells(&mut nodes[4], "phy-reset-duration", &[u32::MAX]);
    assert!(graph::project(&nodes, &bars()).is_err());
    let mut nodes = firmware();
    let mut mdio = node("/pcie/rp1/ethernet@100000/mdio", "", 70);
    set_cells(&mut mdio, "reset-delay-us", &[999]);
    nodes.push(mdio);
    assert!(graph::project(&nodes, &bars()).is_err());
}

#[test]
fn disabled_mac_legacy_fields_remain_opaque_even_when_not_valid_for_linux() {
    for disabled_parent in [false, true] {
        let mut nodes = firmware();
        nodes[4]
            .properties
            .retain(|(name, _)| name != "phy-reset-duration");
        if disabled_parent {
            let mut parent = node("/pcie/rp1/disabled-bus", "simple-bus", 71);
            set(&mut parent, "status", b"disabled\0");
            nodes[4].path = "/pcie/rp1/disabled-bus/ethernet@100000".into();
            nodes[5].path = "/pcie/rp1/disabled-bus/ethernet@100000/ethernet-phy@1".into();
            nodes.push(parent);
        } else {
            set(&mut nodes[4], "status", b"disabled\0");
        }
        graph::project(&nodes, &bars()).unwrap().with_nodes(|tree| {
            let mac = find(tree, "ethernet@100000").unwrap();
            assert_eq!(
                cells(property(mac, "phy-reset-gpios")).unwrap(),
                [46, 32, 1]
            );
            assert!(
                mac.children
                    .iter()
                    .any(|node| node.name == "ethernet-phy@1")
            );
            assert!(mac.children.iter().all(|node| node.name != "mdio"));
        });
    }
}

#[test]
fn upstream_mdio_binding_already_present_is_preserved() {
    let mut nodes = firmware();
    nodes[4]
        .properties
        .retain(|(name, _)| !name.starts_with("phy-reset-"));
    nodes[5].path = "/pcie/rp1/ethernet@100000/mdio/ethernet-phy@1".into();
    let mut mdio = node("/pcie/rp1/ethernet@100000/mdio", "", 70);
    set_cells(&mut mdio, "#address-cells", &[1]);
    set_cells(&mut mdio, "#size-cells", &[0]);
    set_cells(&mut mdio, "reset-gpios", &[46, 32, 1]);
    set_cells(&mut mdio, "reset-delay-us", &[7000]);
    nodes.push(mdio);
    graph::project(&nodes, &bars()).unwrap().with_nodes(|tree| {
        let mdio = find(tree, "mdio").unwrap();
        assert_eq!(property(mdio, "reset-delay-us"), 7000u32.to_be_bytes());
        assert_eq!(property(mdio, "phandle"), 70u32.to_be_bytes());
        assert_eq!(mdio.children.len(), 1);
        assert_eq!(property(&mdio.children[0], "phandle"), 51u32.to_be_bytes());
    });
}
