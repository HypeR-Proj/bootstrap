// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Admit only native guest architectures; ITB metadata owns image identity.

use super::Configuration;
use crate::image::{Architecture, ConfiguredImage, GuestImage};

pub fn configure(image: GuestImage, config: &Configuration) -> Result<ConfiguredImage, String> {
    let host = match std::env::consts::ARCH {
        "aarch64" => Architecture::Aarch64,
        "riscv64" => Architecture::Riscv64,
        "x86_64" => Architecture::X86_64,
        _ => return Err("unsupported host architecture".into()),
    };
    image
        .configure_for_host(config.image_configuration()?, host)
        .map_err(|error| match error {
            crate::image::ConfigurationError::ArchitectureMismatch { image, host } => {
                format!("VM architecture mismatch: ITB {image:?}, host {host:?}")
            }
            other => format!("invalid VM configuration for ITB architecture: {other:?}"),
        })
}

#[cfg(test)]
#[path = "../../tests/image.rs"]
mod tests;
