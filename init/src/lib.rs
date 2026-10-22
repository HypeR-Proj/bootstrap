// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap service policy and supervision; independent of system init.
//!
//! Parsing and planning deliberately depend on no operating-system binding.
//! The runtime adapter must obtain real root-`Directory` and process-construction
//! capabilities before it can apply a validated launch plan.
//! Stage1 prepares a reversible root switch before launching its successor.
//! Core supervision and the transition owner then move in one rendezvous; only
//! the successor commits the root and starts formal system services.

pub mod bootstrap_policy;
pub mod diagnostics;
pub mod manifest;
pub mod profile;
pub mod supervision;

pub mod runtime;

#[path = "../../contracts/mod.rs"]
pub mod contracts;

#[cfg(test)]
#[path = "../../../../tests/app/native-init-payload.rs"]
mod native_payload;

#[path = "../../logging.rs"]
pub mod logging;
