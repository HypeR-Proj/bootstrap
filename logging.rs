// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap-only diagnostic headers, stamped before console buffering.

use core::fmt;

pub struct Prefix<'a> {
    nanoseconds: Option<u64>,
    service: &'a str,
}

/// Captures the boot clock once, using the kernel's seconds/microseconds
/// format. A missing clock is explicit; it must not suppress failure diagnostics.
pub fn prefix(service: &str) -> Prefix<'_> {
    #[cfg(target_os = "hyper")]
    let nanoseconds = hyper_os::time::monotonic_now()
        .ok()
        .map(|time| time.as_nanoseconds());
    #[cfg(not(target_os = "hyper"))]
    let nanoseconds = None;
    Prefix {
        nanoseconds,
        service,
    }
}

impl fmt::Display for Prefix<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ns) = self.nanoseconds {
            write!(
                formatter,
                "[{:>5}.{:06}]",
                ns / 1_000_000_000,
                ns / 1_000 % 1_000_000
            )?;
        } else {
            formatter.write_str("[    ?.??????]")?;
        }
        write!(formatter, "[HypeR bootstrap stage 1][{}] ", self.service)
    }
}
