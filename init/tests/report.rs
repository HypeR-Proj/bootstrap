// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::{ProcessReport, RECORD_BYTES, TRUNCATED, emit};
use hyper_os::task::{ProcessInfo, ProcessPhase, ProcessTermination};

#[test]
fn termination_is_published_as_one_complete_record() {
    let service = "x".repeat(63);
    let report = ProcessReport(Some(ProcessInfo {
        phase: ProcessPhase::Stopped,
        terminal: Some(ProcessTermination::Fault {
            class: u32::MAX,
            code: u64::MAX,
        }),
    }));
    let mut writes = Vec::new();
    emit(
        format_args!("critical service '{service}' terminated: {report}\n"),
        |bytes| writes.push(bytes.to_vec()),
    );
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0],
        format!(
            "[    ?.??????][HypeR bootstrap stage 1][init] critical service '{service}' \
             terminated: reason=fault class=4294967295 code=18446744073709551615\n"
        )
        .as_bytes()
    );
    assert!(writes[0].len() <= RECORD_BYTES);
}

#[test]
fn oversized_diagnostic_is_bounded_and_marks_truncation() {
    let text = "界".repeat(RECORD_BYTES);
    emit(
        format_args!("{text}: suffix must not escape the record\n"),
        |bytes| {
            assert!(bytes.len() <= RECORD_BYTES);
            assert!(bytes.ends_with(TRUNCATED));
            assert!(core::str::from_utf8(bytes).is_ok());
            assert!(!bytes.windows(6).any(|window| window == b"suffix"));
        },
    );
}
