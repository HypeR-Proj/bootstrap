// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap-owned scheduling policy for the resident I/O appliance.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scheduling {
    /// Omit to retain the kernel-selected policy and its configured defaults.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_policy"
    )]
    pub policy: Option<Policy>,
    pub band: u8,
    pub role: Role,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Policy {
    Fair {
        weight: u16,
        quantum_ns: u64,
        #[serde(default)]
        backend: FairBackend,
    },
    Budget {
        budget_ns: u64,
        period_ns: u64,
        burst_ns: u64,
    },
    Fifo {
        priority: u8,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Ordinary,
    Priority,
    Protected,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FairBackend {
    Stride,
    RoundRobin,
    #[default]
    Default,
}

impl Scheduling {
    /// The live kernel additionally enforces capability bounds and the common
    /// budget period. A valid JSON declaration grants no scheduling authority.
    pub fn validate(self) -> Result<(), &'static str> {
        let valid = match self.policy {
            None | Some(Policy::Fifo { .. }) => true,
            Some(Policy::Fair {
                weight, quantum_ns, ..
            }) => (1..=1024).contains(&weight) && (1..=1_000_000_000).contains(&quantum_ns),
            Some(Policy::Budget {
                budget_ns,
                period_ns,
                burst_ns,
            }) => {
                (1..=1_000_000_000).contains(&period_ns)
                    && (1..=period_ns).contains(&budget_ns)
                    && (1..=period_ns).contains(&burst_ns)
            }
        };
        if valid {
            Ok(())
        } else {
            Err("invalid VM scheduling parameters")
        }
    }

    pub fn apply(
        self,
        vcpu: hyper_os::handle::HandleRef<'_, hyper_os::handle::VirtualCpuObject>,
    ) -> hyper_os::Result<()> {
        use hyper_os::scheduling as os;
        self.validate()
            .map_err(|_| hyper_os::Error::InvalidResponse)?;
        let policy = match self.policy {
            None => os::vcpu(vcpu)?.policy,
            Some(Policy::Fair {
                weight,
                quantum_ns,
                backend,
            }) => os::Policy::Fair {
                weight,
                quantum_ns,
                backend: match backend {
                    FairBackend::Stride => os::FairBackend::Stride,
                    FairBackend::RoundRobin => os::FairBackend::RoundRobin,
                    FairBackend::Default => os::FairBackend::Default,
                },
            },
            Some(Policy::Budget {
                budget_ns,
                period_ns,
                burst_ns,
            }) => os::Policy::Budget {
                budget_ns,
                period_ns,
                burst_ns,
            },
            Some(Policy::Fifo { priority }) => os::Policy::Fifo { priority },
        };
        os::set_vcpu(
            vcpu,
            os::Parameters {
                policy,
                band: self.band,
                role: match self.role {
                    Role::Ordinary => os::Role::Ordinary,
                    Role::Priority => os::Role::Priority,
                    Role::Protected => os::Role::Protected,
                },
            },
        )
    }
}

/// Optional configuration fields are absent or populated, never JSON null.
pub(super) fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Scheduling>, D::Error> {
    Scheduling::deserialize(deserializer).map(Some)
}
fn present_policy<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Policy>, D::Error> {
    Policy::deserialize(deserializer).map(Some)
}
