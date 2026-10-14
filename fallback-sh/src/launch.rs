// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Capability-preserving pipeline construction and foreground supervision.
use super::{CommandAuthorities, Error, WORKING_DIRECTORY_RIGHTS, write_terminal};
use crate::contracts::stdio::{Inherited, StandardStream};
use crate::contracts::{filesystem, process};
use hyper_fallback_sh::command::CommandLine;
use hyper_fallback_sh::program::{ImageSource, Program};
use hyper_os::handle::{Rights, RightsOffer};
use hyper_os::startup;
use hyper_os::task::{ProcessBuilder, ProcessInfo, ProcessTermination};
use hyper_os::{Error as OsError, Status};

fn prepare_command(
    command: &CommandLine,
    source: &[u8],
    authorities: &CommandAuthorities,
    child: ChildChannels,
) -> Result<ProcessBuilder, Error> {
    let name = command.argument(0).ok_or(Error::InvalidCommand)?;
    let program =
        Program::resolve(name, &authorities.current_path).map_err(|_| Error::InvalidCommand)?;
    let image_directory = match program.source {
        ImageSource::Environment => &authorities.environment,
        ImageSource::Namespace => &authorities.root_directory,
    };
    let executable = match image_directory.open(
        &program.path,
        crate::contracts::process::EXECUTABLE_FILE_RIGHTS,
    ) {
        Ok(executable) => executable,
        Err(open_error) => {
            let reason = if matches!(open_error, OsError::Status(Status::NOT_FOUND)) {
                "command not found"
            } else {
                "cannot open command"
            };
            write_terminal(
                format!(
                    "bootstrap-stage1-fallback-sh: {reason}: {name:?} (input: \"{}\"; {open_error:?})\r\n",
                    source.escape_ascii()
                )
                .as_bytes(),
            )?;
            return Err(Error::InvalidCommand);
        }
    };
    // Recovery launches only static executables through its retained loader.
    // Explicit paths do not gain formal-shell privileges or library authority.
    let loader = &authorities.loader;
    let builder = ProcessBuilder::create(
        authorities.factory.as_handle_ref(),
        authorities.group.as_handle_ref(),
        authorities.domain.as_handle_ref(),
        executable.as_handle_ref(),
        loader.as_handle_ref(),
    )
    .map_err(Error::from)?;
    builder.add_handle_duplicate(
        loader.as_handle_ref(),
        process::CHILD_LOADER.as_raw(),
        RightsOffer::Exact(process::CHILD_LOADER_CONTRACT.allowed_rights()),
    )?;
    builder.set_name(name).map_err(Error::from)?;
    for index in 0..command.len() {
        builder
            .add_argument(command.argument(index).ok_or(Error::InvalidCommand)?)
            .map_err(Error::from)?;
    }
    builder
        .add_handle_duplicate(
            authorities.library_directory.as_handle_ref(),
            startup::DYNAMIC_LIBRARY_DIRECTORY.as_raw(),
            RightsOffer::Exact(
                Rights::READ
                    .union(Rights::EXECUTE)
                    .union(Rights::PROTECT)
                    .union(Rights::DUPLICATE)
                    .union(Rights::TRANSFER),
            ),
        )
        .map_err(|_| Error::InvalidCommand)?;
    builder
        .add_handle_duplicate(
            authorities.current_directory.as_handle_ref(),
            process::WORKING_DIRECTORY.as_raw(),
            RightsOffer::Exact(WORKING_DIRECTORY_RIGHTS.as_rights()),
        )
        .map_err(|_| Error::InvalidCommand)?;

    builder
        .add_handle_duplicate(
            authorities.root_directory.as_handle_ref(),
            startup::ROOT_DIRECTORY.as_raw(),
            RightsOffer::Exact(WORKING_DIRECTORY_RIGHTS.as_rights()),
        )
        .map_err(|_| Error::InvalidCommand)?;
    builder
        .add_handle_duplicate(
            authorities.factory.as_handle_ref(),
            startup::TASK_FACTORY.as_raw(),
            RightsOffer::Exact(
                Rights::CREATE_PROCESS
                    .union(Rights::DUPLICATE)
                    .union(Rights::TRANSFER),
            ),
        )
        .map_err(|_| Error::InvalidCommand)?;
    builder
        .add_handle_duplicate(
            authorities.group.as_handle_ref(),
            startup::TASK_GROUP.as_raw(),
            RightsOffer::Exact(
                Rights::TASK_GROUP_ATTACH_PROCESS
                    .union(Rights::DUPLICATE)
                    .union(Rights::TRANSFER),
            ),
        )
        .map_err(|_| Error::InvalidCommand)?;
    builder
        .add_handle_duplicate(
            authorities.domain.as_handle_ref(),
            startup::RESOURCE_DOMAIN.as_raw(),
            RightsOffer::Exact(
                Rights::RESOURCE_DOMAIN_SPONSOR
                    .union(Rights::DUPLICATE)
                    .union(Rights::TRANSFER),
            ),
        )
        .map_err(|_| Error::InvalidCommand)?;
    child.input.add_to(&builder, StandardStream::Input)?;
    child.output.add_to(&builder, StandardStream::Output)?;
    if program.filesystem_authority
        && let Some(control) = authorities.filesystem_control.as_ref()
    {
        builder
            .add_handle_duplicate(
                control.as_handle_ref(),
                filesystem::CONTROL_CLIENT.as_raw(),
                RightsOffer::Exact(Rights::WAIT.union(Rights::WRITE)),
            )
            .map_err(|_| Error::InvalidCommand)?;
    }
    child.error.add_to(&builder, StandardStream::Error)?;
    builder.seal().map_err(Error::from)?;
    Ok(builder)
}

/// The fallback shell shares terminal queues directly with its sole foreground
/// child. It performs no eager read while the child owns terminal input.
pub(super) fn launch_command(
    command: &CommandLine,
    source: &[u8],
    authorities: &CommandAuthorities,
    input: &Inherited,
    output: &Inherited,
    error: &Inherited,
) -> Result<(), Error> {
    let builder = prepare_command(
        command,
        source,
        authorities,
        ChildChannels {
            input: input.duplicate(StandardStream::Input)?,
            output: output.duplicate(StandardStream::Output)?,
            error: error.duplicate(StandardStream::Error)?,
        },
    )?;
    let child = builder
        .start()
        .map_err(|failure| Error::from(failure.error()))?;
    child
        .as_process_supervisor()
        .wait_terminated(hyper_os::DEADLINE_INFINITE)?;
    if !process_succeeded(child.as_process_supervisor().info()?) {
        write_terminal(b"bootstrap-stage1-fallback-sh: command failed\r\n")?;
    }
    Ok(())
}

fn process_succeeded(info: ProcessInfo) -> bool {
    matches!(
        info.terminal,
        Some(
            ProcessTermination::ThreadExited { status: 0 }
                | ProcessTermination::ProcessExited { status: 0 }
                | ProcessTermination::LastThreadExited { status: 0 }
        )
    )
}

struct ChildChannels {
    input: Inherited,
    output: Inherited,
    error: Inherited,
}
