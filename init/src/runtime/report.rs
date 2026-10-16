// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Allocation-free init diagnostics.

use crate::diagnostics::ProcessReport;
use core::fmt::{self, Write};
use hyper_os::handle::{ConsoleObject, OwnedHandle, ProcessObject};

use super::LaunchError;

pub(super) fn write(output: &OwnedHandle<ConsoleObject>, message: core::fmt::Arguments<'_>) {
    emit(message, |bytes| {
        let _ = output.as_emergency_console().write_all(bytes);
    });
}

pub(super) fn report_operation_failure(
    output: &OwnedHandle<ConsoleObject>,
    operation: &str,
    error: &impl core::fmt::Debug,
) {
    write(output, format_args!("{operation}: {error:?}\n"));
}

pub(super) fn report_service_launch_failure(
    output: &OwnedHandle<ConsoleObject>,
    service: &str,
    error: &LaunchError,
) {
    write(
        output,
        format_args!("failed to launch service '{service}': {}\n", error.reason()),
    );
}

pub(super) fn report_service_termination(
    output: &OwnedHandle<ConsoleObject>,
    service: &str,
    critical: bool,
    supervisor: &OwnedHandle<ProcessObject>,
) {
    let qualifier = if critical { "critical " } else { "" };
    let report = ProcessReport(supervisor.as_process_supervisor().info().ok());
    write(
        output,
        format_args!("{qualifier}service '{service}' terminated: {report}\n"),
    );
}

// Fit one current Console transfer batch. Formatting directly to the Console
// exposes every write_str fragment to unrelated bootstrap writers.
const RECORD_BYTES: usize = 256;
const TRUNCATED: &[u8] = b"...[truncated]\n";

struct Record {
    bytes: [u8; RECORD_BYTES],
    length: usize,
}

impl fmt::Write for Record {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut count = text.len().min(RECORD_BYTES - TRUNCATED.len() - self.length);
        while !text.is_char_boundary(count) {
            count -= 1;
        }
        self.bytes[self.length..self.length + count].copy_from_slice(&text.as_bytes()[..count]);
        self.length += count;
        if count != text.len() {
            return Err(fmt::Error);
        }
        Ok(())
    }
}

fn emit(message: fmt::Arguments<'_>, output: impl FnOnce(&[u8])) {
    let mut record = Record {
        bytes: [0; RECORD_BYTES],
        length: 0,
    };
    if write!(record, "{}{message}", crate::logging::prefix("init")).is_err() {
        record.bytes[record.length..record.length + TRUNCATED.len()].copy_from_slice(TRUNCATED);
        record.length += TRUNCATED.len();
    }
    output(&record.bytes[..record.length]);
}

#[cfg(test)]
#[path = "../../tests/report.rs"]
mod tests;
