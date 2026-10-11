// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use hyper_os::device::{self, FirmwareField, IrqTrigger};
use hyper_os::handle::{DeviceAssignmentAuthorityObject, HandleRef};
use hyper_os::{Error, Result, Status};

#[derive(Clone, Copy, Debug)]
pub(crate) struct MmioResource {
    pub(crate) base: u64,
    pub(crate) length: u64,
}
impl MmioResource {
    pub(crate) fn start(self) -> u64 {
        self.base
    }
    pub(crate) fn size(self) -> u64 {
        self.length
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FirmwareNode {
    pub(crate) id: u32,
    pub(crate) path: String,
    pub(crate) compatible: Vec<String>,
    pub(crate) registers: Vec<MmioResource>,
    pub(crate) properties: Vec<(String, Vec<u8>)>,
    pub(crate) kernel_owned: bool,
    pub(crate) interrupt: Option<(u32, bool)>,
}
impl FirmwareNode {
    pub(crate) fn id(&self) -> u32 {
        self.id
    }
    pub(crate) fn path(&self) -> &str {
        &self.path
    }
    pub(crate) fn is_compatible(&self, value: &str) -> bool {
        self.compatible.iter().any(|item| item == value)
    }
    pub(crate) fn registers(&self) -> &[MmioResource] {
        &self.registers
    }
    pub(crate) fn kernel_claimed(&self) -> bool {
        self.kernel_owned
    }
    pub(crate) fn property(&self, name: &str) -> Option<&[u8]> {
        self.properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_slice())
    }
}

// Bound untrusted firmware-derived allocation and work; this is discovery,
// never the I/O hot path. Snapshot reads do not claim any resource.
const MAX_NODES: u32 = 4096;
const MAX_FIELD: usize = 65536;
const MAX_BYTES: usize = 8 * 1024 * 1024;

fn field(
    authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
    node: u32,
    kind: FirmwareField,
    name: &str,
    bytes: &mut usize,
) -> Result<Vec<u8>> {
    let size = device::firmware_read(authority, node, kind, name, &mut [])?;
    if size > MAX_FIELD || size > MAX_BYTES.saturating_sub(*bytes) {
        return Err(Error::Status(Status::RESOURCE_LIMIT));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| Error::Status(Status::NO_MEMORY))?;
    output.resize(size, 0);
    if size != 0 && device::firmware_read(authority, node, kind, name, &mut output)? != size {
        return Err(Error::InvalidResponse);
    }
    *bytes += size;
    Ok(output)
}

pub(crate) fn read(
    authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
    names: &[&str],
) -> Result<Vec<FirmwareNode>> {
    read_properties(authority, Some(names))
}

/// Capture complete property records for bounded board-firmware projection.
pub(crate) fn read_all(
    authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
) -> Result<Vec<FirmwareNode>> {
    read_properties(authority, None)
}

fn read_properties(
    authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
    names: Option<&[&str]>,
) -> Result<Vec<FirmwareNode>> {
    let mut nodes = Vec::new();
    let mut bytes = 0;
    for id in 0..=MAX_NODES {
        let info = match device::firmware_info(authority, id) {
            Ok(info) => info,
            Err(Error::Status(Status::NOT_FOUND)) => return Ok(nodes),
            Err(error) => return Err(error),
        };
        if id == MAX_NODES {
            return Err(Error::Status(Status::RESOURCE_LIMIT));
        }
        let path = field(authority, id, FirmwareField::Path, "", &mut bytes)?;
        let path = String::from_utf8(path).map_err(|_| Error::InvalidResponse)?;
        let compatible = field(authority, id, FirmwareField::Compatible, "", &mut bytes)?;
        let compatible = if compatible.is_empty() {
            Vec::new()
        } else {
            let text = core::str::from_utf8(&compatible).map_err(|_| Error::InvalidResponse)?;
            let text = text.strip_suffix('\0').ok_or(Error::InvalidResponse)?;
            text.split('\0').map(String::from).collect()
        };
        let registers = field(authority, id, FirmwareField::Registers, "", &mut bytes)?;
        if registers.len() != info.register_count as usize * 16 {
            return Err(Error::InvalidResponse);
        }
        let registers = registers
            .chunks_exact(16)
            .map(|record| {
                Ok(MmioResource {
                    base: u64::from_le_bytes(
                        record[..8].try_into().map_err(|_| Error::InvalidResponse)?,
                    ),
                    length: u64::from_le_bytes(
                        record[8..].try_into().map_err(|_| Error::InvalidResponse)?,
                    ),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let all_names = if names.is_none() {
            let bytes = field(authority, id, FirmwareField::PropertyNames, "", &mut bytes)?;
            property_names(&bytes)?
        } else {
            Vec::new()
        };
        let names: Vec<&str> = names.map_or_else(
            || all_names.iter().map(String::as_str).collect(),
            |names| names.to_vec(),
        );
        let mut properties = Vec::new();
        properties
            .try_reserve_exact(names.len())
            .map_err(|_| Error::Status(Status::NO_MEMORY))?;
        for name in &names {
            match field(authority, id, FirmwareField::Property, name, &mut bytes) {
                Ok(value) => properties.push((String::from(*name), value)),
                Err(Error::Status(Status::NOT_FOUND)) => {}
                Err(error) => return Err(error),
            }
        }
        nodes
            .try_reserve(1)
            .map_err(|_| Error::Status(Status::NO_MEMORY))?;
        nodes.push(FirmwareNode {
            id,
            path,
            compatible,
            registers,
            properties,
            kernel_owned: info.kernel_owned,
            interrupt: info
                .interrupt
                .map(|irq| (irq.number, irq.trigger == IrqTrigger::Level)),
        });
    }
    Err(Error::Status(Status::RESOURCE_LIMIT))
}

fn property_names(bytes: &[u8]) -> Result<Vec<String>> {
    let text = core::str::from_utf8(bytes).map_err(|_| Error::InvalidResponse)?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = text
        .strip_suffix('\0')
        .ok_or(Error::InvalidResponse)?
        .split('\0')
        .map(String::from)
        .collect();
    // Firmware catalogues such as __symbols__ are larger than a projected
    // peripheral node. The input field and whole snapshot have separate byte
    // bounds; the guest projection enforces its own smaller property budget.
    if names.len() > 4096
        || names.iter().enumerate().any(|(index, name)| {
            name.is_empty()
                || name.len() > device::PROPERTY_NAME_MAX_BYTES
                || names[..index].contains(name)
        })
    {
        return Err(Error::InvalidResponse);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_catalogue_is_not_limited_to_projected_peripheral_size() {
        let catalogue: String = (0..334).map(|index| format!("symbol_{index}\0")).collect();
        let names = property_names(catalogue.as_bytes()).unwrap();
        assert_eq!(names.len(), 334);
        assert_eq!(names[333], "symbol_333");
        for bad in [
            b"duplicate\0duplicate\0".as_slice(),
            b"unterminated",
            b"a\0\0",
        ] {
            assert!(property_names(bad).is_err());
        }
        let excessive: String = (0..4097).map(|index| format!("p{index}\0")).collect();
        assert!(property_names(excessive.as_bytes()).is_err());
    }
}
