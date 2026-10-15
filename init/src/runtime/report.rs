// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Allocation-free init diagnostics.

use crate::diagnostics::write_process_termination;
use hyper_os::handle::{ConsoleObject, OwnedHandle, ProcessObject};

use super::LaunchError;

pub(super) fn write(output: &OwnedHandle<ConsoleObject>, message: core::fmt::Arguments<'_>) {
    use core::fmt::Write;

    struct Writer<'a>(&'a OwnedHandle<ConsoleObject>);
    impl core::fmt::Write for Writer<'_> {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            self.0
                .as_emergency_console()
                .write_all(text.as_bytes())
                .map_err(|_| core::fmt::Error)
        }
    }
    let _ = write!(
        Writer(output),
        "{}{message}",
        crate::logging::prefix("init")
    );
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
    let console = output.as_emergency_console();
    write(output, format_args!("failed to launch service '"));
    let _ = console.write_all(service.as_bytes());
    let _ = console.write_all(b"': ");
    let _ = console.write_all(error.reason());
    let _ = console.write_all(b"\n");
}

pub(super) fn report_service_termination(
    output: &OwnedHandle<ConsoleObject>,
    service: &str,
    critical: bool,
    supervisor: &OwnedHandle<ProcessObject>,
) {
    let console = output.as_emergency_console();
    if critical {
        write(output, format_args!("critical service '"));
    } else {
        write(output, format_args!("service '"));
    }
    let _ = console.write_all(service.as_bytes());
    let _ = console.write_all(b"' terminated: ");
    match supervisor.as_process_supervisor().info() {
        Ok(info) => {
            write_process_termination(info, |fragment| {
                let _ = console.write_all(fragment);
            });
        }
        Err(_) => {
            let _ = console.write_all(b"process-info-unavailable");
        }
    }
    let _ = console.write_all(b"\n");
}
