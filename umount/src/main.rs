// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Graceful unmount client. Forced and lazy detach are deliberately unsupported.

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!(
            "usage: umount TARGET\nSynchronize and detach an idle managed filesystem. Open files, directories\nand nested mounts return Busy; no forced or lazy unmount is supported."
        );
        return Ok(());
    }
    let [target] = args.as_slice() else {
        return Err("expected one mount target; see umount --help".into());
    };
    if target.starts_with('-') || target.contains('\0') {
        return Err("invalid mount target".into());
    }
    let request = format!("umount\0{target}\0");
    let mut startup = hyper_rt::process::startup().map_err(|error| format!("{error:?}"))?;
    let (ok, response) =
        hyper_service::filesystem::control::execute(&mut startup, request.as_bytes())
            .map_err(|error| format!("mount service unavailable: {error:?}"))?;
    if ok {
        print!("{response}");
        Ok(())
    } else {
        Err(response)
    }
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("umount: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
