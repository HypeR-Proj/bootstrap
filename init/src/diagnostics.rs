// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Standard integer formatting and rendering of supervised Process termination details.

use core::fmt;
use hyper_os::task::{ProcessInfo, ProcessTermination};

/// Formats a lifecycle report into its caller's record without allocating or
/// publishing individual fragments to the shared Console.
pub(crate) struct ProcessReport(pub Option<ProcessInfo>);

impl fmt::Display for ProcessReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(info) = self.0 else {
            return formatter.write_str("process-info-unavailable");
        };
        match info.terminal {
            Some(ProcessTermination::Requested) => formatter.write_str("reason=requested"),
            Some(ProcessTermination::ThreadExited { status }) => {
                write!(formatter, "reason=thread-exited status={status}")
            }
            Some(ProcessTermination::ProcessExited { status }) => {
                write!(formatter, "reason=process-exited status={status}")
            }
            Some(ProcessTermination::LastThreadExited { status }) => {
                write!(formatter, "reason=last-thread-exited status={status}")
            }
            Some(ProcessTermination::Fault { class, code }) => {
                write!(formatter, "reason=fault class={class} code={code}")
            }
            Some(ProcessTermination::TaskGroupStop { generation }) => {
                write!(formatter, "reason=task-group-stop generation={generation}")
            }
            None => formatter.write_str("reason=unavailable"),
        }
    }
}

#[cfg(test)]
#[path = "../tests/diagnostics.rs"]
mod tests;
