// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! User-facing filesystem manager client; no raw device authority is granted.

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut operands = Vec::new();
    let mut all = false;
    let mut table = crate::contracts::filesystem::control::SYSTEM_FSTAB.to_string();
    let mut explicit_table = false;
    let mut format = "auto".to_string();
    let mut options = "defaults".to_string();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: mount [-T FSTAB] -a\n       mount [-T FSTAB] SOURCE_OR_TARGET\n       mount [-t auto|fat|erofs] [-o ro|rw] SOURCE TARGET\nWithout arguments, list mounted Native volumes. The default bootstrap table is /etc/fstab.\nUse -T PATH to select another table during recovery."
                );
                return Ok(());
            }
            "-a" => all = true,
            "-T" | "--fstab" => {
                table = args.next().ok_or("-T requires an absolute fstab path")?;
                explicit_table = true;
            }
            "-t" => format = args.next().ok_or("-t requires a filesystem type")?,
            "-o" => options = args.next().ok_or("-o requires mount options")?,
            value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
            _ => operands.push(argument),
        }
    }
    let fields: Vec<&str> = if all {
        if !operands.is_empty() || format != "auto" || options != "defaults" {
            return Err("-a cannot be combined with a source or mount override".into());
        }
        vec!["all", &table]
    } else {
        match operands.as_slice() {
            [] if !explicit_table && format == "auto" && options == "defaults" => vec!["list"],
            [entry] if format == "auto" && options == "defaults" => vec!["entry", &table, entry],
            [source, target] if !explicit_table => vec!["mount", source, target, &format, &options],
            _ => {
                return Err(
                    "expected SOURCE TARGET, one fstab source/target, or -a; see mount --help"
                        .into(),
                );
            }
        }
    };
    let mut request = Vec::new();
    for field in fields {
        if field.contains('\0') {
            return Err("invalid argument".into());
        }
        request.extend_from_slice(field.as_bytes());
        request.push(0);
    }
    let mut startup = hyper_rt::process::startup().map_err(|error| format!("{error:?}"))?;
    let (ok, response) = crate::contracts::filesystem::control::execute(&mut startup, &request)
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
            eprintln!("bootstrap-stage1-mount: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
#[path = "../../contracts/mod.rs"]
mod contracts;
