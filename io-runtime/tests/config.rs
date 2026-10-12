// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;
use hyper_os::device::{FirmwareIdentity, Profile};
use serde_json::{Value, json};

fn board() -> Value {
    json!({
        "format": "hyper.board.v1",
        "architecture": "aarch64",
        "boot": "qemu-direct",
        "virtual-machines": [],
        "io-vm": {
            "runtime": "io-runtime",
            "name": "storage.backend-1",
            "image": "/vm/appliances/storage.itb",
            "configuration": {
                "memory-bytes": 134217728,
                "vcpus": 1,
                "bootargs": "console=ttyAMA0 hyper.role=io",
                "affinity": [{"vcpu": 0, "cpus": [1, 3]}]
            },
            "io-device": {"profile": "virtio-mmio-scsi", "compatible": "virtio,mmio"}
        }
    })
}

fn parse(board: &Value) -> Result<Config, String> {
    Config::parse(&serde_json::to_vec(board).unwrap())
}

#[test]
fn bounded_reader_accepts_the_selected_file_without_namespace_lookup() {
    let bytes = serde_json::to_vec(&board()).unwrap();
    let config = Config::read(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(config.definition().image, "/vm/appliances/storage.itb");

    let mut input = std::io::Cursor::new(vec![b' '; MAX_BOARD_BYTES * 2]);
    assert!(Config::read(&mut input).is_err());
    assert_eq!(input.position(), MAX_BOARD_BYTES as u64 + 1);
}

#[test]
fn one_node_supplies_image_identity_runtime_policy_and_device() {
    let config = parse(&board()).unwrap();
    let definition = config.definition();
    assert_eq!(definition.name, "storage.backend-1");
    assert_eq!(definition.image, "/vm/appliances/storage.itb");
    assert_eq!(definition.configuration.memory_bytes, 128 * 1024 * 1024);
    assert_eq!(definition.configuration.vcpus, 1);
    assert_eq!(
        definition.configuration.bootargs,
        "console=ttyAMA0 hyper.role=io"
    );
    assert_eq!(definition.configuration.affinity[0].vcpu, 0);
    assert_eq!(definition.configuration.affinity[0].cpus, vec![1, 3]);
    assert!(config.network_device().is_none());
    assert_eq!(config.device().profile(), Profile::VirtioMmioScsi);
    assert!(matches!(
        config.device().identity(),
        FirmwareIdentity::Compatible("virtio,mmio")
    ));
}

#[test]
fn physical_firmware_path_belongs_to_the_same_io_node() {
    let mut source = board();
    source["io-vm"]["io-device"] = json!({
        "profile": "bcm2712-sdhci", "path": "/soc@107c000000/mmc@fff000"
    });
    let config = parse(&source).unwrap();
    assert_eq!(config.device().profile(), Profile::Userspace);
    assert!(matches!(
        config.device().identity(),
        FirmwareIdentity::FdtPath("/soc@107c000000/mmc@fff000")
    ));
}

#[test]
fn missing_io_node_has_no_fleet_or_legacy_device_fallback() {
    let mut source = board();
    let node = source.as_object_mut().unwrap().remove("io-vm").unwrap();
    source["io-device"] = node["io-device"].clone();
    source["virtual-machines"] = json!([{
        "name": "io", "image": "/vm/io.itb", "configuration": node["configuration"]
    }]);
    assert!(parse(&source).is_err());
}

#[test]
fn wrong_owner_or_unrelated_guest_lifecycle_fields_are_rejected() {
    for owner in [json!("vm-runtime"), json!("vm-manager"), Value::Null] {
        let mut source = board();
        source["io-vm"]["runtime"] = owner;
        assert!(parse(&source).is_err());
    }
    for field in ["runtime", "name", "image", "configuration", "io-device"] {
        let mut source = board();
        source["io-vm"].as_object_mut().unwrap().remove(field);
        assert!(parse(&source).is_err(), "missing {field}");
    }
    for field in ["autostart", "disk", "device"] {
        let mut source = board();
        source["io-vm"][field] = json!(false);
        assert!(parse(&source).is_err(), "unknown {field}");
    }
}

#[test]
fn resident_layout_and_resource_limits_are_checked_before_loading() {
    for memory in [64 * 1024 * 1024, 256 * 1024 * 1024] {
        let mut source = board();
        source["io-vm"]["configuration"]["memory-bytes"] = json!(memory);
        assert!(parse(&source).is_err());
    }
    for cpus in [0, 2, 8] {
        let mut source = board();
        source["io-vm"]["configuration"]["vcpus"] = json!(cpus);
        assert!(parse(&source).is_err());
    }
}

#[test]
fn affinity_is_validated_in_the_shared_vm_policy() {
    for affinity in [
        json!([{"vcpu": 1, "cpus": [0]}]),
        json!([{"vcpu": 0, "cpus": []}]),
        json!([{"vcpu": 0, "cpus": [1, 1]}]),
        json!([{"vcpu": 0, "cpus": [256]}]),
        json!([{"vcpu": 0, "cpus": [0]}, {"vcpu": 0, "cpus": [1]}]),
    ] {
        let mut source = board();
        source["io-vm"]["configuration"]["affinity"] = affinity;
        assert!(parse(&source).is_err());
    }
    let mut source = board();
    source["io-vm"]["configuration"]
        .as_object_mut()
        .unwrap()
        .remove("affinity");
    assert!(
        parse(&source)
            .unwrap()
            .definition()
            .configuration
            .affinity
            .is_empty()
    );
}

#[test]
fn image_must_be_a_canonical_bootstrap_path() {
    for image in [
        "/data/io.itb",
        "vm/io.itb",
        "/vm/../io.itb",
        "/vm//io.itb",
        "/vm/./io.itb",
        "/vm/io",
        "/vm/io.itb/",
        "/vm/a\\b.itb",
        "/vm/io\n.itb",
    ] {
        let mut source = board();
        source["io-vm"]["image"] = json!(image);
        assert!(parse(&source).is_err(), "{image}");
    }
    let mut source = board();
    source["io-vm"]["image"] = json!(format!("/vm/{}.itb", "x".repeat(505)));
    assert!(parse(&source).is_err());
}

#[test]
fn names_and_board_identity_are_validated() {
    for name in [
        "",
        "_io",
        "two words",
        "i/o",
        "é",
        "abcdefghijklmnopqrstuvwxyz1234567",
    ] {
        let mut source = board();
        source["io-vm"]["name"] = json!(name);
        assert!(parse(&source).is_err());
    }
    for (field, value) in [("format", "hyper.vm-config"), ("architecture", "riscv64")] {
        let mut source = board();
        source[field] = json!(value);
        assert!(parse(&source).is_err());
    }
}

#[test]
fn board_read_bound_is_enforced_even_for_valid_json() {
    let mut bytes = serde_json::to_vec(&board()).unwrap();
    bytes.resize(MAX_BOARD_BYTES, b' ');
    assert!(Config::parse(&bytes).is_ok());
    bytes.push(b' ');
    assert!(Config::parse(&bytes).is_err());
}

fn network_board() -> Value {
    let mut source = board();
    source["io-vm"]["network-device"] = json!({
        "profile": "virtio-mmio-net", "compatible": "virtio,mmio"
    });
    source["io-vm"]["networks"] = json!([{
        "name": "default", "bridge": "hbr0", "uplink": "eth0"
    }]);
    source
}

#[test]
fn optional_network_controller_has_its_own_profile_and_firmware_identity() {
    let config = parse(&network_board()).unwrap();
    let network = config.network_device().unwrap();
    assert_eq!(config.device().profile(), Profile::VirtioMmioScsi);
    assert_eq!(network.profile(), Profile::VirtioMmioNet);
    assert!(matches!(
        network.identity(),
        FirmwareIdentity::Compatible("virtio,mmio")
    ));
    let mut source = network_board();
    source["io-vm"]["network-device"] = json!({
        "profile": "virtio-mmio-net", "path": "/virtio_mmio@a003c00"
    });
    assert!(matches!(
        parse(&source).unwrap().network_device().unwrap().identity(),
        FirmwareIdentity::FdtPath("/virtio_mmio@a003c00")
    ));
}

#[test]
fn network_controller_and_deployment_must_be_present_together() {
    for field in ["network-device", "networks"] {
        let mut source = network_board();
        source["io-vm"].as_object_mut().unwrap().remove(field);
        assert!(parse(&source).is_err());
        source["io-vm"][field] = Value::Null;
        assert!(parse(&source).is_err());
    }
    let mut source = board();
    source["io-vm"]["network-device"] = Value::Null;
    source["io-vm"]["networks"] = Value::Null;
    assert!(parse(&source).is_err());
}

#[test]
fn network_attachment_rejects_storage_profiles_and_unsupported_boards() {
    for profile in ["bcm2712-sdhci", "virtio-mmio-scsi", "unknown"] {
        let mut source = network_board();
        source["io-vm"]["network-device"]["profile"] = json!(profile);
        assert!(parse(&source).is_err());
    }
    let mut source = network_board();
    source["io-vm"]["io-device"]["profile"] = json!("virtio-mmio-net");
    assert!(parse(&source).is_err());
    for boot in [json!("rpi5-native"), Value::Null, json!("unknown")] {
        let mut source = network_board();
        source["boot"] = boot;
        assert!(parse(&source).is_err());
    }
}

#[test]
fn network_deployment_requires_one_named_bridge_with_distinct_uplink() {
    for networks in [
        json!([]),
        json!([{"name":"a","bridge":"hbr0","uplink":"eth0"},
               {"name":"b","bridge":"hbr1","uplink":"eth1"}]),
        json!([{"name":"a","bridge":"eth0","uplink":"eth0"}]),
        json!([{"name":"a","bridge":"hbr0"}]),
        json!([{"name":"a","bridge":"hbr0","uplink":"eth0","extra":true}]),
    ] {
        let mut source = network_board();
        source["io-vm"]["networks"] = networks;
        assert!(parse(&source).is_err());
    }
    for name in [
        "",
        "A",
        "has space",
        "a.b",
        "abcdefghijklmnopqrstuvwxyz123456",
    ] {
        let mut source = network_board();
        source["io-vm"]["networks"][0]["name"] = json!(name);
        assert!(parse(&source).is_err(), "{name}");
    }
    for interface in ["", "0eth", "eth0.1", "with space", "abcdefghijklmnop"] {
        for field in ["bridge", "uplink"] {
            let mut source = network_board();
            source["io-vm"]["networks"][0][field] = json!(interface);
            assert!(parse(&source).is_err(), "{field} {interface}");
        }
    }
}

#[test]
fn rp1_uplink_requires_pi_firmware_and_cannot_be_a_storage_controller() {
    let mut source = network_board();
    source["boot"] = json!("rpi5-firmware");
    source["io-vm"]["io-device"] =
        json!({"profile":"bcm2712-sdhci", "compatible":"brcm,bcm2712-sdhci"});
    source["io-vm"]["network-device"] = json!({"profile":"pci-function", "pci-id":"1de4:0001"});
    let config = parse(&source).unwrap();
    assert_eq!(
        config.network_device().unwrap().profile(),
        Profile::PciFunction
    );
    assert!(matches!(
        config.network_device().unwrap().identity(),
        FirmwareIdentity::PciId {
            vendor: 0x1de4,
            device: 1
        }
    ));
    source["boot"] = json!("qemu-direct");
    assert!(parse(&source).is_err());
    source["boot"] = json!("rpi5-firmware");
    source["io-vm"]["io-device"] = source["io-vm"]["network-device"].clone();
    assert!(parse(&source).is_err());
    let mut source = network_board();
    source["boot"] = json!("rpi5-firmware");
    assert!(parse(&source).is_err());
}
