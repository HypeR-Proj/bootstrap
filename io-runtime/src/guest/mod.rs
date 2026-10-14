// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Resident I/O VM configuration and image preparation.

mod affinity;
#[cfg(any(test, all(target_os = "hyper", target_arch = "aarch64")))]
mod copy;
#[cfg(any(test, all(target_os = "hyper", target_arch = "aarch64")))]
mod image;
#[cfg(all(target_os = "hyper", target_arch = "aarch64"))]
pub mod install;
pub mod profile;
#[cfg(any(test, all(target_os = "hyper", target_arch = "aarch64")))]
mod read_cache;

use affinity::Affinity;
use serde::Deserialize;

/// The resident appliance identity; guest fleet policy belongs to system init.
pub struct Definition {
    pub name: String,
    pub image: String,
    pub configuration: Configuration,
}

impl Definition {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.len() > 32
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || !self.name.as_bytes()[0].is_ascii_alphanumeric()
        {
            return Err("VM names must start with a letter or digit and contain at most 32 letters, digits, '-', '_' or '.'".into());
        }
        if !self.image.starts_with('/')
            || self.image.len() > 512
            || self.image.chars().any(char::is_control)
            || self.image.split('/').any(|part| part == "..")
        {
            return Err("VM image must be an absolute path without parent components or control characters (at most 512 bytes)".into());
        }
        self.configuration.image_configuration()?;
        Ok(())
    }
}

/// Explicit per-instance policy, stored in JSON rather than the image bundle.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    #[serde(rename = "memory-bytes")]
    pub memory_bytes: u64,
    pub vcpus: u32,
    pub bootargs: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affinity: Vec<Affinity>,
}
impl Configuration {
    pub fn image_configuration(&self) -> Result<crate::image::Configuration, String> {
        use crate::image::BootArguments;
        // Architecture-specific limits are checked after reading the ITB.
        if !(1..=8).contains(&self.vcpus) {
            return Err("VM requires 1..=8 vCPUs".into());
        }
        affinity::masks(&self.affinity, self.vcpus)?;
        if self.memory_bytes < 64 * 1024 * 1024 || !self.memory_bytes.is_power_of_two() {
            return Err("VM memory-bytes must be a power of two of at least 64 MiB".into());
        }
        let boot_arguments = BootArguments::new(&self.bootargs)
            .map_err(|_| "VM bootargs must contain at most 2048 bytes and no NUL".to_string())?;
        Ok(crate::image::Configuration {
            memory_size: self.memory_bytes,
            vcpu_count: self.vcpus,
            boot_arguments,
        })
    }
}
