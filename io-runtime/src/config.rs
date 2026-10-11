// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! The resident I/O VM and its device come from one bounded board snapshot.

use crate::device_policy::Policy;
use hyper_vm_policy::fleet::{Configuration, Definition};
use serde::Deserialize;
use std::io::Read;

const MAX_BOARD_BYTES: usize = 1024 * 1024;
const RESIDENT_RAM_BYTES: u64 = 128 * 1024 * 1024;

// The image builder validates the complete board schema. This service owns the
// I/O node; unrelated disk, deployment-file and business-VM fields stay opaque.
#[derive(Deserialize)]
struct Board {
    format: String,
    architecture: String,
    boot: Option<String>,
    #[serde(rename = "io-vm")]
    io_vm: IoVm,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IoVm {
    runtime: Runtime,
    name: String,
    image: String,
    configuration: Configuration,
    #[serde(rename = "io-device")]
    device: Policy,
    #[serde(rename = "network-device", default, deserialize_with = "present")]
    network_device: Option<Policy>,
    #[serde(default, deserialize_with = "present")]
    networks: Option<Vec<Network>>,
}

/// A present optional field must carry its declared value, never JSON null.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Network {
    name: String,
    bridge: String,
    uplink: String,
}

fn validate_networks(device: Option<&Policy>, networks: Option<&[Network]>) -> Result<(), String> {
    let network = match (device, networks) {
        (None, None) => return Ok(()),
        (Some(device), Some([network]))
            if matches!(
                device.profile(),
                hyper_os::device::Profile::VirtioMmioNet | hyper_os::device::Profile::PciFunction
            ) =>
        {
            network
        }
        _ => return Err("network deployment requires one supported uplink and one bridge".into()),
    };
    if !hyper_service::io::valid_name(&network.name) {
        return Err("invalid I/O VM network name".into());
    }
    for interface in [&network.bridge, &network.uplink] {
        let bytes = interface.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 15
            || !bytes[0].is_ascii_alphabetic()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
        {
            return Err("invalid Linux network interface name".into());
        }
    }
    if network.bridge == network.uplink {
        return Err("network bridge and uplink must be different interfaces".into());
    }
    Ok(())
}

#[derive(Deserialize)]
enum Runtime {
    #[serde(rename = "io-runtime")]
    IoRuntime,
}

pub struct Config {
    definition: Definition,
    device: Policy,
    network_device: Option<Policy>,
}

impl Config {
    pub fn read(input: impl Read) -> Result<Self, String> {
        let mut bytes = Vec::new();
        input
            .take(MAX_BOARD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BOARD_BYTES {
            return Err("board configuration exceeds 1 MiB".into());
        }
        let board: Board = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if board.format != "hyper.board.v1" || board.architecture != "aarch64" {
            return Err("unsupported board format or architecture".into());
        }
        let IoVm {
            runtime: Runtime::IoRuntime,
            name,
            image,
            configuration,
            device,
            network_device,
            networks,
        } = board.io_vm;
        if matches!(
            device.profile(),
            hyper_os::device::Profile::VirtioMmioNet | hyper_os::device::Profile::PciFunction
        ) {
            return Err("I/O storage device requires a storage controller profile".into());
        }
        validate_networks(network_device.as_ref(), networks.as_deref())?;
        if let Some(network) = network_device.as_ref() {
            let expected_boot = match network.profile() {
                hyper_os::device::Profile::VirtioMmioNet => "qemu-direct",
                hyper_os::device::Profile::PciFunction => "rpi5-firmware",
                _ => return Err("unsupported physical network profile".into()),
            };
            if board.boot.as_deref() != Some(expected_boot) {
                return Err("physical network profile does not match board boot method".into());
            }
        }
        let definition = Definition {
            name,
            image,
            configuration,
            autostart: false,
            disk: None,
            network: None,
        };
        definition.validate()?;
        // The backend must boot before /data exists. Its image is packaged in
        // the bootstrap archive, independently of the configuration volume.
        if !definition.image.starts_with("/vm/")
            || !definition.image.ends_with(".itb")
            || definition.image.contains('\\')
            || definition
                .image
                .split('/')
                .skip(1)
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err("I/O VM image must be a canonical /vm/...itb bootstrap path".into());
        }
        if definition.configuration.memory_bytes != RESIDENT_RAM_BYTES
            || definition.configuration.vcpus != 1
        {
            return Err("resident I/O VM requires 128 MiB and one vCPU".into());
        }
        Ok(Self {
            definition,
            device,
            network_device,
        })
    }

    pub fn definition(&self) -> &Definition {
        &self.definition
    }

    pub fn device(&self) -> &Policy {
        &self.device
    }

    pub fn network_device(&self) -> Option<&Policy> {
        self.network_device.as_ref()
    }
}

#[cfg(test)]
#[path = "../tests/config.rs"]
mod tests;
