// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Native-owned partition selections. The backend receives only checked ranges.

use serde::Deserialize;
use std::io::Read;

pub mod catalog;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    pub client: u32,
    pub source: String,
    pub read_only: bool,
}

#[derive(Default, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub native: Vec<Volume>,
    pub guests: Vec<Volume>,
}

#[cfg(target_os = "hyper")]
pub fn load(environment: &hyper_os::fs::Directory) -> Result<Policy, String> {
    let file = match environment.open("etc/hyper/storage.json", hyper_os::fs::FileRights::READ) {
        Ok(file) => file,
        Err(hyper_os::Error::Status(hyper_os::Status::NOT_FOUND)) => return Ok(Policy::default()),
        Err(error) => return Err(format!("open storage policy: {error}")),
    };
    read(file.into_std())
}

pub fn read(input: impl Read) -> Result<Policy, String> {
    let mut bytes = Vec::new();
    input
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> Result<Policy, String> {
    if bytes.len() > 8192 {
        return Err("storage policy exceeds 8 KiB".into());
    }
    let policy: Policy = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let volumes: Vec<_> = policy.native.iter().chain(&policy.guests).collect();
    if volumes.len() >= hyper_os::vm::IO_MAX_CLIENTS {
        return Err("too many block clients".into());
    }
    for (index, volume) in volumes.iter().enumerate() {
        if volume.client == 0
            || volume.client as usize >= hyper_os::vm::IO_MAX_CLIENTS
            || hyper_block::Selector::parse(&volume.source).is_none()
            || volumes[..index].iter().any(|old| {
                old.client == volume.client
                    || (old.source == volume.source
                        && (!old.read_only || !volume.read_only || index < policy.native.len()))
            })
        {
            return Err("invalid or duplicate partition binding".into());
        }
    }
    Ok(policy)
}

impl Policy {
    /// Broker and storage policy must describe the same guest disk clients.
    /// Native ranges never enter the guest admission table.
    pub fn validate_broker(&self, clients: &[crate::clients::Client]) -> Result<(), String> {
        for volume in &self.native {
            if clients.iter().any(|client| client.id == volume.client) {
                return Err("Native storage client also exposed through guest broker".into());
            }
        }
        for volume in &self.guests {
            if !clients
                .iter()
                .any(|client| client.id == volume.client && client.volume.is_some())
            {
                return Err("guest disk has no matching broker client".into());
            }
        }
        for client in clients.iter().filter(|client| client.volume.is_some()) {
            if !self.guests.iter().any(|volume| volume.client == client.id) {
                return Err("broker disk has no authorized partition binding".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/storage_policy.rs"]
mod tests;
