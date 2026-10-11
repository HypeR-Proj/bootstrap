// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Board firmware for the whole RP1 PCI function. Linux owns every peripheral
//! driver; this module only preserves firmware and translates its bus addresses.

mod graph;
mod normalize;
mod references;
mod tree;

use crate::firmware;
use hyper_os::handle::{DeviceAssignmentAuthorityObject, HandleRef};
use hyper_os::{Error, Result};
use hyper_vm_image::guest_fdt::io::PciBar;
pub use tree::Projection;

/// Reading firmware confers no device authority. The independently claimed PCI
/// function supplies the only BARs which can appear in this projection.
pub fn describe(
    authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
    bars: &[PciBar],
) -> Result<Projection> {
    graph::project(&firmware::read_all(authority)?, bars)
}

fn cells(bytes: &[u8]) -> Result<Vec<u32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::InvalidResponse);
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|part| u32::from_be_bytes([part[0], part[1], part[2], part[3]]))
        .collect())
}
fn cell(node: &firmware::FirmwareNode, name: &str) -> Result<u32> {
    let bytes: [u8; 4] = node
        .property(name)
        .ok_or(Error::InvalidResponse)?
        .try_into()
        .map_err(|_| Error::InvalidResponse)?;
    Ok(u32::from_be_bytes(bytes))
}
fn enabled(node: &firmware::FirmwareNode) -> bool {
    matches!(node.property("status"), None | Some(b"ok\0" | b"okay\0"))
}
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}
fn descendant(path: &str, root: &str) -> bool {
    path.strip_prefix(root)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
#[path = "../../tests/rp1_profile.rs"]
mod tests;
