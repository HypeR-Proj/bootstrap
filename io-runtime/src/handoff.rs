// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Versioned, capability-backed transfer of an already running I/O appliance.
//! No process-local handle value or virtual address crosses this boundary.

use crate::clients::Client;
use serde::{Deserialize, Serialize};

pub const ENDPOINT: u32 = 0x8008_0010;
pub const DESCRIPTION: u32 = 0x8008_0012;
pub const READY: u32 = 0x8008_0020;
pub const DESCRIBE_MESSAGE: &[u8] = b"HYPER-IO-DESCRIBE/1";
pub const ADOPT_MESSAGE: &[u8] = b"HYPER-IO-ADOPT/1";
pub const READY_MESSAGE: &[u8] = b"HYPER-IO-READY/1";
pub const MAX_DESCRIPTION: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkerMode {
    None,
    Sdhci,
    VirtioTest,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    Machine,
    Cpu,
    Serial,
    SerialBuffer,
    StorageDevice,
    NetworkDevice,
    Broker,
    NativeMemory,
    NativeMailbox,
    BrokerMailbox,
}
impl Kind {
    pub fn required_rights(self) -> hyper_os::Rights {
        use hyper_os::Rights;
        let owner = Rights::TRANSFER;
        match self {
            Self::Machine => owner
                .union(Rights::WRITE)
                .union(Rights::WAIT)
                .union(Rights::INSPECT)
                .union(Rights::REQUEST_STOP),
            Self::Cpu => owner
                .union(Rights::WRITE)
                .union(Rights::WAIT)
                .union(Rights::INSPECT)
                .union(Rights::START),
            Self::Serial => owner.union(Rights::READ).union(Rights::WAIT),
            Self::SerialBuffer => owner.union(Rights::READ).union(Rights::MAP),
            Self::StorageDevice | Self::NetworkDevice => owner.union(Rights::INSPECT),
            Self::Broker => owner.union(Rights::READ).union(Rights::WAIT),
            Self::NativeMemory => owner.union(Rights::MAP),
            Self::NativeMailbox | Self::BrokerMailbox => owner
                .union(Rights::READ)
                .union(Rights::WRITE)
                .union(Rights::WAIT),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub kind: Kind,
    pub rights: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Description {
    pub version: u32,
    pub name: String,
    pub image: String,
    pub worker: WorkerMode,
    pub cpus: usize,
    pub native: usize,
    pub network: bool,
    pub broker: bool,
    pub clients: Vec<Client>,
    pub slots: Vec<Slot>,
}
impl Description {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || self.name.is_empty()
            || self.name.len() > 64
            || self.image.len() > 1024
            || !(1..=8).contains(&self.cpus)
            || self.native > hyper_os::vm::IO_MAX_CLIENTS
            || self.clients.len() >= hyper_os::vm::IO_MAX_CLIENTS
            || (!self.broker && !self.clients.is_empty())
        {
            return Err("invalid I/O handoff description");
        }
        let mut kinds = vec![Kind::Machine];
        kinds.extend(std::iter::repeat_n(Kind::Cpu, self.cpus));
        kinds.extend([Kind::Serial, Kind::SerialBuffer, Kind::StorageDevice]);
        if self.network {
            kinds.push(Kind::NetworkDevice);
        }
        if self.broker {
            kinds.push(Kind::Broker);
        }
        kinds.extend(std::iter::repeat_n(Kind::NativeMemory, self.native));
        kinds.extend(std::iter::repeat_n(Kind::NativeMailbox, self.native.max(1)));
        kinds.extend(std::iter::repeat_n(Kind::BrokerMailbox, self.clients.len()));
        if kinds.len() > 64
            || kinds.len() != self.slots.len()
            || kinds
                .iter()
                .zip(&self.slots)
                .any(|(kind, slot)| *kind != slot.kind)
        {
            return Err("invalid I/O capability inventory");
        }
        if self.slots.iter().any(|slot| {
            hyper_os::Rights::from_bits(slot.rights)
                .is_none_or(|rights| !rights.contains(slot.kind.required_rights()))
        }) {
            return Err("insufficient I/O owner rights");
        }
        if self.worker != WorkerMode::None {
            let physical = self
                .slots
                .iter()
                .find(|slot| slot.kind == Kind::StorageDevice)
                .ok_or("missing userspace physical device")?;
            let required = hyper_os::Rights::WRITE
                .union(hyper_os::Rights::WAIT)
                .union(hyper_os::Rights::DUPLICATE);
            if hyper_os::Rights::from_bits(physical.rights)
                .is_none_or(|rights| !rights.contains(required))
            {
                return Err("insufficient physical worker rights");
            }
        }
        for (index, client) in self.clients.iter().enumerate() {
            if client.id == 0
                || client.id as usize >= hyper_os::vm::IO_MAX_CLIENTS
                || client.connection().encode().is_none()
                || self.clients[..index]
                    .iter()
                    .any(|other| client.conflicts_with(other))
            {
                return Err("invalid I/O client policy");
            }
        }
        Ok(())
    }
}

/// Cursor and prefix state are captured after the last predecessor read.
pub fn encode_commit(cursor: u64, line_start: bool) -> Vec<u8> {
    let mut bytes = ADOPT_MESSAGE.to_vec();
    bytes.extend(cursor.to_le_bytes());
    bytes.push(u8::from(line_start));
    bytes
}
pub fn decode_commit(bytes: &[u8]) -> Result<(u64, bool), &'static str> {
    let suffix = bytes
        .strip_prefix(ADOPT_MESSAGE)
        .ok_or("invalid I/O adoption record")?;
    let [a, b, c, d, e, f, g, h, line] = suffix else {
        return Err("invalid I/O adoption length");
    };
    if *line > 1 {
        return Err("invalid I/O log state");
    }
    Ok((
        u64::from_le_bytes([*a, *b, *c, *d, *e, *f, *g, *h]),
        *line != 0,
    ))
}

#[cfg(test)]
#[path = "../tests/handoff.rs"]
mod tests;
