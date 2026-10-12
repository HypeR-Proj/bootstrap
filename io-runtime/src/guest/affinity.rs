// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Default host CPU placement, applied while all guest vCPUs are dormant.

use serde::{Deserialize, Serialize};

pub const MAX_HOST_CPUS: usize = hyper_os::vm::VCPU_AFFINITY_MAX_WORDS * 64;
pub type CpuMask = [u64; hyper_os::vm::VCPU_AFFINITY_MAX_WORDS];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Affinity {
    pub vcpu: u32,
    pub cpus: Vec<u32>,
}

/// Validate the complete policy before any placement side effects.
pub fn masks(entries: &[Affinity], vcpus: u32) -> Result<Vec<(u32, CpuMask)>, String> {
    if entries.len() > vcpus as usize {
        return Err("too many vCPU affinity entries".into());
    }
    let mut result = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if entry.vcpu >= vcpus
            || entries[..index]
                .iter()
                .any(|other| other.vcpu == entry.vcpu)
        {
            return Err(format!(
                "affinity vCPU {} is out of range or duplicated",
                entry.vcpu
            ));
        }
        if entry.cpus.is_empty() || entry.cpus.len() > MAX_HOST_CPUS {
            return Err(format!(
                "affinity for vCPU {} requires a nonempty bounded CPU list",
                entry.vcpu
            ));
        }
        let mut words = [0; hyper_os::vm::VCPU_AFFINITY_MAX_WORDS];
        for &cpu in &entry.cpus {
            let cpu = cpu as usize;
            if cpu >= MAX_HOST_CPUS {
                return Err(format!("affinity host CPU must be below {MAX_HOST_CPUS}"));
            }
            let bit = 1u64 << (cpu % 64);
            if words[cpu / 64] & bit != 0 {
                return Err(format!("affinity host CPU {cpu} is duplicated"));
            }
            words[cpu / 64] |= bit;
        }
        result.push((entry.vcpu, words));
    }
    Ok(result)
}

/// A rejected CPU mask fails startup; callers must not start any guest vCPU.
#[cfg(any(test, all(target_os = "hyper", target_arch = "aarch64")))]
pub fn apply(
    entries: &[Affinity],
    vcpus: u32,
    mut set: impl FnMut(u32, &[u64]) -> hyper_os::Result<()>,
) -> Result<(), String> {
    for (vcpu, words) in masks(entries, vcpus)? {
        set(vcpu, &words)
            .map_err(|error| format!("default affinity for vCPU {vcpu} rejected: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/affinity.rs"]
mod tests;
