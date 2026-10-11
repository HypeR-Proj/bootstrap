// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap service supervisor entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    let Ok(mut startup) = hyper_rt::process::startup() else {
        return ExitCode::FAILURE;
    };
    match hyper_service_manager::runtime::run(&mut startup) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            use std::io::Write;
            let _ = std::io::stderr().write_all(error.diagnostic());
            ExitCode::FAILURE
        }
    }
}
