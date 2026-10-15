// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod clients;

pub mod storage_policy;

pub mod config;

pub mod device_policy;

pub mod deadline;

mod firmware;

#[cfg(all(target_os = "hyper", target_arch = "aarch64"))]
pub mod mmio;
pub mod sdhci;

pub mod rp1;

pub mod pci;
pub mod pcie;

pub mod guest_log;

pub mod guest;
pub mod protocol;

pub mod handoff;

pub mod image;

pub mod partition;

#[path = "../../contracts/mod.rs"]
pub mod contracts;

#[path = "../../logging.rs"]
pub mod logging;
