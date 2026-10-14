// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Optional VM-manager client delegation by the recovery shell.
//! Bootstrap never launches or provisions the formal fleet manager.

use crate::contracts::StartupContract;
use hyper_os::handle::{ByteChannelObject, CapabilityChannelObject, Rights};
use hyper_os::startup::StartupPurpose;

pub const MANAGER_CONNECTION_NAME: &str = "vm.manager-connection";
pub const CLIENT_CONTROL_NAME: &str = "vm.client-control";
pub const CLIENT_CAPABILITIES_NAME: &str = "vm.client-capabilities";

pub const MANAGER_CONNECTION: StartupPurpose<CapabilityChannelObject> =
    StartupPurpose::new(0x8005_0007);
pub const CLIENT_CONTROL: StartupPurpose<ByteChannelObject> = StartupPurpose::new(0x8005_0008);
pub const CLIENT_CAPABILITIES: StartupPurpose<CapabilityChannelObject> =
    StartupPurpose::new(0x8005_0009);

pub const CLIENT_CONTROL_CONTRACT: StartupContract = StartupContract::exact(
    CLIENT_CONTROL_NAME,
    CLIENT_CONTROL,
    Rights::WAIT.union(Rights::READ).union(Rights::WRITE),
);
pub const CLIENT_CAPABILITIES_CONTRACT: StartupContract = StartupContract::exact(
    CLIENT_CAPABILITIES_NAME,
    CLIENT_CAPABILITIES,
    Rights::WAIT.union(Rights::READ),
);
/// A shell or console manager may pass this client connector to another trusted
/// shell. Individual vmm processes receive dedicated channels instead.
pub const DELEGATED_CLIENT_CONNECTION_CONTRACT: StartupContract = StartupContract::exact(
    MANAGER_CONNECTION_NAME,
    MANAGER_CONNECTION,
    Rights::WAIT
        .union(Rights::WRITE)
        .union(Rights::DUPLICATE)
        .union(Rights::TRANSFER),
);
/// Connects a shell-launched client to the system VM manager.
/// The receiver supplies the control and capability channels atomically.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagerConnectionRequest;

impl ManagerConnectionRequest {
    pub const fn encode(self) -> [u8; 8] {
        [b'H', b'V', b'M', 1, 5, 1, 0, 0]
    }
}
