// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Filesystem deployment configuration and runtime worker selection.

pub mod admission;
pub mod config;
pub mod worker;

pub mod format;
pub mod fstab;

pub mod transfer;

#[path = "../../contracts/mod.rs"]
pub mod contracts;

// The executable providers share this dispatcher by source path. Compile it in
// the backend's host tests as well so protocol additions cannot bypass checking.
#[cfg(test)]
#[allow(dead_code)]
mod worker_transport;
