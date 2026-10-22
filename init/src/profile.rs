// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap policy selected by name, independently of the kernel boot contract.

use crate::bootstrap_policy::{BootstrapAuthority, BootstrapPolicy};
use crate::manifest::{self, AuthorityPolicy, LaunchPlan, Manifest};
use serde::Deserialize;

pub const DIRECTORY: &str = "/etc/hyper/boot-profiles";
pub const DEFAULT: &str = "default";
pub const MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(rename = "SPDX-FileCopyrightText")]
    _copyright: Option<String>,
    #[serde(rename = "SPDX-License-Identifier")]
    _license: Option<String>,
    version: u32,
    pub services: String,
    pub io: IoSource,
    pub handoff: Option<Handoff>,
    #[serde(default)]
    pub requires: Vec<String>,
}

/// A provider's implementation is selected by its manifest service, not by its
/// executable name or possession of device authority. Its successor is admitted
/// independently against the formal environment's component policy.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum IoSource {
    None {},
    IoVm {
        service: String,
        successor: Option<String>,
    },
    Service {
        service: String,
        successor: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Handoff {
    pub system_init: String,
    pub system_root: String,
    pub put_old: String,
}

impl IoSource {
    pub fn service(&self) -> Option<&str> {
        match self {
            Self::None {} => None,
            Self::IoVm { service, .. } | Self::Service { service, .. } => Some(service),
        }
    }

    pub fn successor(&self) -> Option<&str> {
        match self {
            Self::None {} => None,
            Self::IoVm { successor, .. } | Self::Service { successor, .. } => successor.as_deref(),
        }
    }
}

impl Profile {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_BYTES {
            return Err("boot profile exceeds size limit".into());
        }
        let profile: Self = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if profile.version != 1 || !absolute(&profile.services) {
            return Err("invalid boot profile version or manifest path".into());
        }
        if profile.requires.len() > 128
            || profile.requires.iter().any(|path| !absolute(path))
            || profile
                .requires
                .iter()
                .enumerate()
                .any(|(index, path)| profile.requires[..index].iter().any(|other| other == path))
        {
            return Err("invalid or duplicate required boot path".into());
        }
        if let Some(name) = profile.io.service()
            && !identifier(name)
        {
            return Err("invalid storage provider service name".into());
        }
        if let Some(next) = &profile.handoff {
            if !absolute(&next.system_init)
                || !absolute(&next.system_root)
                || !absolute(&next.put_old)
                || next.system_root.len() + next.put_old.len() > hyper_os::fs::MAX_PATH_BYTES
            {
                return Err("invalid root handoff paths".into());
            }
            if profile.io.service().is_some() && profile.io.successor().is_none() {
                return Err("storage handoff requires a successor component".into());
            }
        } else if profile.io.successor().is_some() {
            return Err("storage successor requires root handoff".into());
        }
        if profile.io.successor().is_some_and(|path| !absolute(path)) {
            return Err("invalid storage successor component path".into());
        }
        Ok(profile)
    }

    pub fn storage_service(&self, manifest: &Manifest<'_>) -> Result<Option<usize>, String> {
        self.io
            .service()
            .map(|name| {
                manifest
                    .services()
                    .position(|service| service.name() == name)
                    .ok_or_else(|| format!("storage provider {name:?} is absent from the manifest"))
            })
            .transpose()
    }

    /// Validate the source selection before any service receives authority.
    /// Unselected programs cannot create an I/O VM or impersonate readiness.
    pub fn validate(&self, manifest: &Manifest<'_>) -> Result<LaunchPlan, String> {
        let provider = self.storage_service(manifest)?;
        let image = provider
            .and_then(|index| manifest.service(index))
            .map(|service| service.image());
        let policy = ProfilePolicy {
            profile: self,
            image,
        };
        let plan = manifest::validate(manifest, &policy).map_err(|error| format!("{error:?}"))?;
        let ready = crate::bootstrap_policy::io_ready_service(&plan)
            .map_err(|error| format!("invalid storage readiness: {error:?}"))?;
        let filesystems = crate::bootstrap_policy::filesystem_enabled(&plan)
            .map_err(|error| format!("invalid filesystem provider pair: {error:?}"))?;
        // An I/O VM can provide only guest backends while another service has
        // already prepared an independent root. Only the volume stream requires
        // a mount-readiness rendezvous; root switching still verifies ownership.
        if filesystems && ready.is_none() || ready.is_some() && ready != provider {
            return Err("storage readiness must belong to the selected provider".into());
        }
        let io_vm = matches!(self.io, IoSource::IoVm { .. });
        for (index, service) in manifest.services().enumerate() {
            if Some(index) == provider
                && service
                    .capabilities()
                    .any(|grant| matches!(grant.purpose(), "console.system" | "filesystem.attach"))
            {
                return Err(
                    "storage, console and filesystem owners must be separate services".into(),
                );
            }
            if service.image() == crate::bootstrap_policy::IO_RUNTIME_IMAGE
                && (!io_vm || Some(index) != provider)
            {
                return Err("io-runtime must be the selected io-vm provider".into());
            }
        }
        let mut vm_authority = false;
        for index in 0..plan.service_count() {
            for grant in (0..manifest::MAX_CAPABILITIES_PER_SERVICE)
                .filter_map(|capability| plan.capability_grant(index, capability))
            {
                let selected = Some(index) == provider;
                if grant.authority() == BootstrapAuthority::VmAuthority.key() {
                    if !io_vm || !selected {
                        return Err(
                            "VM creation authority requires the selected io-vm provider".into()
                        );
                    }
                    vm_authority = true;
                }
                if matches!(
                    BootstrapAuthority::from_key(grant.authority()),
                    Some(
                        BootstrapAuthority::IoBrokerServer
                            | BootstrapAuthority::FilesystemProvider
                            | BootstrapAuthority::DeviceAuthority
                    )
                ) && !selected
                {
                    return Err("storage capability belongs to an unselected provider".into());
                }
            }
        }
        if io_vm && !vm_authority {
            return Err("io-vm provider requires VM creation authority".into());
        }
        crate::supervision::validate(manifest).map_err(|error| format!("{error:?}"))?;
        Ok(plan)
    }
}

struct ProfilePolicy<'a> {
    profile: &'a Profile,
    image: Option<&'a str>,
}

impl AuthorityPolicy for ProfilePolicy<'_> {
    fn authority<'policy>(
        &'policy self,
        source: &str,
    ) -> Option<manifest::AuthorityDeclaration<'policy>> {
        BootstrapPolicy.authority(source)
    }

    fn startup_purpose(
        &self,
        image: &str,
        name: &str,
    ) -> Option<manifest::StartupPurposeDeclaration> {
        if Some(image) == self.image {
            let contract = crate::contracts::io::STARTUP_CONTRACTS
                .iter()
                .copied()
                .find(|contract| {
                    contract.name() == name
                        && (matches!(self.profile.io, IoSource::IoVm { .. })
                            || name != "vm.creation-authority")
                });
            if let Some(contract) = contract {
                return Some(manifest::StartupPurposeDeclaration {
                    value: contract.purpose(),
                    object_kind: contract.object_kind(),
                    required_rights: contract.required_rights().bits(),
                    allowed_rights: contract.allowed_rights().bits(),
                });
            }
        }
        BootstrapPolicy.startup_purpose(image, name)
    }

    fn right(&self, name: &str) -> Option<u64> {
        BootstrapPolicy.right(name)
    }
}

/// Unknown selectors never fall back to another profile. Names cannot escape
/// the profile directory, and duplicate selectors are rejected as ambiguous.
pub fn select(command_line: &str) -> Result<&str, String> {
    let mut selected = None;
    for word in command_line.split_ascii_whitespace() {
        if word == "hyper.boot-profile" {
            return Err("hyper.boot-profile requires a name".into());
        }
        let Some(name) = word.strip_prefix("hyper.boot-profile=") else {
            continue;
        };
        if !identifier(name) || selected.replace(name).is_some() {
            return Err("invalid or repeated hyper.boot-profile selector".into());
        }
    }
    Ok(selected.unwrap_or(DEFAULT))
}

fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn absolute(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && path.len() <= hyper_os::fs::MAX_PATH_BYTES
        && !path.bytes().any(|byte| byte.is_ascii_control())
        && path[1..]
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

#[cfg(test)]
#[path = "../tests/profile.rs"]
mod tests;
