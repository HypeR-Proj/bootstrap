// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Explicit board selection and atomic claim of an IRQ-free rng200 register bank.
use crate::firmware::{self, FirmwareNode};
use hyper_os::handle::{
    DeviceAssignmentAuthorityObject, HandleRef, OwnedHandle, PhysicalDeviceObject,
};
use hyper_os::{Error, Result, Status, device};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    profile: Profile,
    compatible: Compatible,
}
#[derive(Deserialize)]
enum Profile {
    #[serde(rename = "bcm2711-rng200")]
    Rng200,
}
#[derive(Deserialize)]
enum Compatible {
    #[serde(rename = "brcm,bcm2711-rng200")]
    Rng200,
}

fn select(nodes: &[FirmwareNode]) -> Result<u32> {
    let mut nodes = nodes
        .iter()
        .filter(|n| n.is_compatible("brcm,bcm2711-rng200"));
    let node = nodes.next().ok_or(Error::Status(Status::NOT_FOUND))?;
    if nodes.next().is_some() {
        return Err(Error::Status(Status::BUSY));
    }
    if node.kernel_claimed()
        || node.interrupt.is_some()
        || node.registers().len() != 1
        || node.registers()[0].size() != crate::rng200::BYTES
        || !node.registers()[0].start().is_multiple_of(4)
    {
        return Err(Error::Status(Status::NOT_SUPPORTED));
    }
    Ok(node.id())
}
impl Policy {
    /// Inspection never touches entropy registers. The atomic bundle claim is
    /// the authority boundary; no physical address is inferred from board type.
    pub fn claim(
        &self,
        authority: HandleRef<'_, DeviceAssignmentAuthorityObject>,
    ) -> Result<OwnedHandle<PhysicalDeviceObject>> {
        let (Profile::Rng200, Compatible::Rng200) = (&self.profile, &self.compatible);
        let node = select(&firmware::read(authority, &[])?)?;
        device::claim_bundle(
            authority,
            &[device::BundleEntry {
                node,
                resource: 0,
                offset: 0,
            }],
            node,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node() -> FirmwareNode {
        FirmwareNode {
            id: 7,
            path: "/soc/rng@8000".into(),
            compatible: vec!["brcm,bcm2711-rng200".into()],
            registers: vec![firmware::MmioResource {
                base: 0x107d208000,
                length: 0x28,
            }],
            properties: vec![],
            kernel_owned: false,
            interrupt: None,
        }
    }
    #[test]
    fn selector_rejects_ambiguity_ownership_and_unexpected_geometry() {
        assert_eq!(select(&[node()]), Ok(7));
        assert!(select(&[]).is_err());
        assert!(select(&[node(), node()]).is_err());
        let mut n = node();
        n.kernel_owned = true;
        assert!(select(&[n]).is_err());
        let mut n = node();
        n.interrupt = Some((32, false));
        assert!(select(&[n]).is_err());
        let mut n = node();
        n.registers[0].length = 0x100;
        assert!(select(&[n]).is_err());
        let mut n = node();
        n.registers[0].base += 1;
        assert!(select(&[n]).is_err());
    }
}
