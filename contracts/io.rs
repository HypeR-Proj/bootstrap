// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Explicit startup authority for the trusted physical I/O VM owner.

use crate::contracts::StartupContract;
use hyper_os::{handle::Rights, startup};

mod connection;

// Each bootstrap owner consumes only its side of these wire contracts.
#[allow(unused_imports)]
pub use connection::{
    CONNECT_BYTES, Connection, DISK, NETWORK, decode_connect, encode_connect, parse_mac,
    valid_name, valid_volume,
};

pub const DEVICE_AUTHORITY_NAME: &str = "io.device-authority";
pub const STARTUP_CONTRACTS: &[StartupContract] = &[
    StartupContract::exact(
        "vm.creation-authority",
        startup::VIRTUAL_MACHINE_CREATION_AUTHORITY,
        Rights::DERIVE.union(Rights::CREATE_VIRTUAL_MACHINE),
    ),
    READY_CONTRACT,
    BROKER_SERVER_CONTRACT,
    crate::contracts::filesystem::PROVIDER_CONTRACT,
    StartupContract::exact(
        DEVICE_AUTHORITY_NAME,
        startup::DEVICE_ASSIGNMENT_AUTHORITY,
        Rights::INSPECT
            .union(Rights::ASSIGN_DEVICE)
            .union(Rights::MAP_DMA),
    ),
];

/// Explicit one-shot result for bootstrap mounts, before formal init takes over.
pub const READY_NAME: &str = "io.ready";
pub const READY: startup::StartupPurpose<hyper_os::handle::ByteChannelObject> =
    startup::StartupPurpose::new(0x8006_0001);
pub const READY_CONTRACT: StartupContract =
    StartupContract::exact(READY_NAME, READY, Rights::WAIT.union(Rights::WRITE));
pub const READY_MESSAGE: &[u8] = b"HYPER-IO-READY/1\n";
pub const READY_TIMEOUT_SECONDS: u64 = 120;

/// The trusted manager delegates individual runtime sessions over this channel.
pub const BROKER_SERVER: startup::StartupPurpose<hyper_os::handle::CapabilityChannelObject> =
    startup::StartupPurpose::new(0x8006_0002);
pub const BROKER_SERVER_CONTRACT: StartupContract = StartupContract::exact(
    "io.broker-server",
    BROKER_SERVER,
    Rights::WAIT.union(Rights::READ).union(Rights::TRANSFER),
);
pub const NETWORK_NOTIFICATION_BASE: u64 = 0x0a20_0000;
pub const NETWORK_NOTIFICATION_STRIDE: u64 = 0x10000;
pub const NETWORK_NOTIFICATION_IRQ_BASE: u32 = 64;
/// Rendezvous has no queued capability ownership; the caller keeps every MOVE
/// source until the exact receive commits, and bounds waiting with one deadline.
pub fn send_capabilities(
    endpoint: &hyper_os::capability_channel::CapabilityChannel,
    bytes: &[u8],
    dispositions: &mut [hyper_os::capability_channel::CapabilityDisposition<'_>],
    deadline: u64,
) -> hyper_os::Result<()> {
    use hyper_os::handle::CapabilityChannelObject;
    use hyper_os::wait::{ObjectSignals, WaitItem, wait_many};
    loop {
        let observation = wait_many(
            &[WaitItem::new(
                endpoint.as_handle_ref(),
                ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
                    .union(ObjectSignals::<CapabilityChannelObject>::PEER_CLOSED),
            )],
            deadline,
        )?;
        if !ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
            .is_present_in(observation.observed)
        {
            return Err(hyper_os::Error::InvalidResponse);
        }
        match endpoint.try_send(bytes, dispositions) {
            Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => {}
            result => return result,
        }
    }
}

#[cfg(test)]
#[path = "tests/io_connection.rs"]
mod connection_tests;
