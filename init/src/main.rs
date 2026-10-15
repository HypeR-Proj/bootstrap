// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap service supervisor entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    let Ok(mut startup) = hyper_rt::process::startup() else {
        return ExitCode::FAILURE;
    };
    match hyper_bootstrap_stage1_init::runtime::run(&mut startup) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            use std::io::Write;
            let _ = write!(
                std::io::stderr(),
                "{}{}",
                hyper_bootstrap_stage1_init::logging::prefix("init"),
                error.diagnostic()
            );
            ExitCode::FAILURE
        }
    }
}
