// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! A physical device selector validated independently of board-file loading.

use hyper_os::device::{FirmwareIdentity, Profile};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(try_from = "Selector")]
pub struct Policy {
    profile: ProfileName,
    identity: Identity,
}

enum Identity {
    Compatible(String),
    Path(String),
    PciId { vendor: u16, device: u16 },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selector {
    profile: ProfileName,
    #[serde(default, deserialize_with = "identity_field")]
    compatible: Option<String>,
    #[serde(default, deserialize_with = "identity_field")]
    path: Option<String>,
    #[serde(rename = "pci-id", default, deserialize_with = "identity_field")]
    pci_id: Option<String>,
}

fn identity_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    // Absence selects the other identity field; an explicit null is malformed.
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
enum ProfileName {
    #[serde(rename = "virtio-mmio-scsi")]
    VirtioScsi,
    #[serde(rename = "virtio-mmio-net")]
    VirtioNet,
    #[serde(rename = "bcm2712-sdhci")]
    Sdhci,
    #[serde(rename = "pci-function")]
    PciFunction,
}

impl TryFrom<Selector> for Policy {
    type Error = String;

    fn try_from(selector: Selector) -> Result<Self, Self::Error> {
        let identity = match (selector.compatible, selector.path, selector.pci_id) {
            (Some(value), None, None) if !matches!(selector.profile, ProfileName::PciFunction) => {
                validate_text(&value)?;
                Identity::Compatible(value)
            }
            (None, Some(value), None)
                if !matches!(selector.profile, ProfileName::PciFunction)
                    && value.starts_with('/')
                    && !value
                        .split('/')
                        .skip(1)
                        .any(|part| matches!(part, "" | "." | "..")) =>
            {
                validate_text(&value)?;
                Identity::Path(value)
            }
            (None, None, Some(value)) if matches!(selector.profile, ProfileName::PciFunction) => {
                let (vendor, device) = parse_pci_id(&value)?;
                Identity::PciId { vendor, device }
            }
            _ => return Err("device requires exactly one identity matching its profile".into()),
        };
        Ok(Self {
            profile: selector.profile,
            identity,
        })
    }
}

fn validate_text(text: &str) -> Result<(), String> {
    if text.is_empty() || text.len() > 512 || text.bytes().any(|byte| byte <= 32 || byte == 127) {
        return Err("invalid firmware identity".into());
    }
    Ok(())
}

fn parse_pci_id(text: &str) -> Result<(u16, u16), String> {
    let bytes = text.as_bytes();
    if bytes.len() != 9
        || bytes[4] != b':'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err("PCI identity must be canonical vvvv:dddd hexadecimal".into());
    }
    let vendor = u16::from_str_radix(&text[..4], 16).map_err(|_| "invalid PCI vendor")?;
    let device = u16::from_str_radix(&text[5..], 16).map_err(|_| "invalid PCI device")?;
    Ok((vendor, device))
}

impl Policy {
    pub const fn profile(&self) -> Profile {
        match self.profile {
            ProfileName::VirtioScsi => Profile::VirtioMmioScsi,
            ProfileName::VirtioNet => Profile::VirtioMmioNet,
            ProfileName::Sdhci => Profile::Userspace,
            ProfileName::PciFunction => Profile::PciFunction,
        }
    }

    pub fn identity(&self) -> FirmwareIdentity<'_> {
        match &self.identity {
            Identity::Compatible(value) => FirmwareIdentity::Compatible(value),
            Identity::Path(value) => FirmwareIdentity::FdtPath(value),
            Identity::PciId { vendor, device } => FirmwareIdentity::PciId {
                vendor: *vendor,
                device: *device,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_unique_selector_required() {
        assert!(
            serde_json::from_str::<Policy>(
                r#"{"profile":"virtio-mmio-scsi","compatible":"virtio,mmio"}"#
            )
            .is_ok()
        );
        for identity in [
            r#""path":"relative""#,
            r#""path":"/soc/../x""#,
            r#""compatible":"x","path":"/x""#,
            r#""compatible":"""#,
            r#""compatible":"x","path":null"#,
            r#""compatible":null,"path":"/soc/device""#,
            r#""compatible":null"#,
            r#""path":null"#,
        ] {
            assert!(
                serde_json::from_str::<Policy>(&format!(
                    r#"{{"profile":"virtio-mmio-scsi",{identity}}}"#
                ))
                .is_err()
            );
        }
    }
}

#[cfg(test)]
mod pci_tests {
    use super::*;

    #[test]
    fn pci_identity_is_typed_and_cannot_be_confused_with_firmware_paths() {
        let policy: Policy =
            serde_json::from_str(r#"{"profile":"pci-function","pci-id":"1de4:0001"}"#).unwrap();
        assert_eq!(policy.profile(), Profile::PciFunction);
        assert!(matches!(
            policy.identity(),
            FirmwareIdentity::PciId {
                vendor: 0x1de4,
                device: 1
            }
        ));
        for value in [
            "",
            "1DE4:0001",
            "1de4:1",
            "1de4-0001",
            " 1de4:0001",
            "1de4:00010",
            "zzzz:0001",
        ] {
            assert!(
                serde_json::from_str::<Policy>(&format!(
                    r#"{{"profile":"pci-function","pci-id":"{value}"}}"#
                ))
                .is_err()
            );
        }
        for value in [
            r#"{"profile":"pci-function","compatible":"pci1de4,1"}"#,
            r#"{"profile":"pci-function","path":"/pcie/rp1"}"#,
            r#"{"profile":"pci-function","pci-id":null}"#,
            r#"{"profile":"pci-function","pci-id":"1de4:0001","path":"/pcie/rp1"}"#,
            r#"{"profile":"virtio-mmio-net","pci-id":"1de4:0001"}"#,
        ] {
            assert!(serde_json::from_str::<Policy>(value).is_err());
        }
    }
}
