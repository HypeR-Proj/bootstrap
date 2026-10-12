// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Capability-preserving pipeline construction and foreground supervision.
use super::{CommandAuthorities, Error, WORKING_DIRECTORY_RIGHTS, write_terminal};
use hyper_fallback_sh::command::CommandLine;
use hyper_fallback_sh::program::{Authority, ImageSource, Program, RuntimeEnvironment};
use hyper_os::capability_channel::{CapabilityChannel, CapabilityDisposition};
use hyper_os::channel;
use hyper_os::handle::{
    ByteChannelObject, CapabilityChannelObject, OwnedHandle, Rights, RightsOffer,
};
use hyper_os::startup;
use hyper_os::task::{ProcessBuilder, ProcessInfo, ProcessTermination};
use hyper_os::wait::{ObjectSignals, WaitItem, wait_many};
use hyper_os::{Error as OsError, Status};
use hyper_service::stdio::{Inherited, StandardStream};
use hyper_service::{filesystem, process, stdio, vm};

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
        hyper_service::process::EXECUTABLE_FILE_RIGHTS,
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
                    "fallback-sh: {reason}: {name:?} (input: \"{}\"; {open_error:?})\r\n",
                    source.escape_ascii()
                )
                .as_bytes(),
            )?;
            return Err(Error::InvalidCommand);
        }
    };
    // Commands outside the recovery tree use the full loader even for static ELF.
    // Keep recovery independent of the mounted system and its loader.
    // Resolve /system only when starting a system command. A directory opened
    // before storage mounts would keep naming the covered RAM directory.
    let system_environment = match program.environment {
        RuntimeEnvironment::Recovery => None,
        RuntimeEnvironment::System => Some(
            process::open_environment(&authorities.root_directory, "/system")
                .map_err(Error::from)?,
        ),
    };
    let system_libraries = system_environment
        .as_ref()
        .map(process::open_library_directory)
        .transpose()
        .map_err(Error::from)?;
    let command_loader = system_environment
        .as_ref()
        .map(|environment| process::open_loader(environment, process::LoaderKind::Dynamic))
        .transpose()
        .map_err(Error::from)?;
    let loader = command_loader.as_ref().unwrap_or(&authorities.loader);
    let builder = ProcessBuilder::create(
        authorities.factory.as_handle_ref(),
        authorities.group.as_handle_ref(),
        authorities.domain.as_handle_ref(),
        executable.as_handle_ref(),
        loader.as_handle_ref(),
    )
    .map_err(Error::from)?;
    if program.is_shell() {
        process::ShellDelegation {
            environment: authorities.environment.as_handle_ref(),
            loader: authorities.loader.as_handle_ref(),
            libraries: authorities.library_directory.as_handle_ref(),
            tasks: authorities.task_inspector.as_handle_ref(),
            objects: authorities.object_inspector.as_handle_ref(),
            memory: authorities.memory_inspector.as_handle_ref(),
            cpus: authorities.cpu_inspector.as_handle_ref(),
            filesystem: authorities
                .filesystem_control
                .as_ref()
                .map(OwnedHandle::as_handle_ref),
            virtual_machines: authorities
                .vm_connection
                .as_ref()
                .map(CapabilityChannel::as_handle_ref),
        }
        .add_to(&builder)?;
    } else {
        builder.add_handle_duplicate(
            loader.as_handle_ref(),
            process::CHILD_LOADER.as_raw(),
            RightsOffer::Exact(process::CHILD_LOADER_CONTRACT.allowed_rights()),
        )?;
    }
    builder.set_name(name).map_err(Error::from)?;
    for index in 0..command.len() {
        builder
            .add_argument(command.argument(index).ok_or(Error::InvalidCommand)?)
            .map_err(Error::from)?;
    }
    builder
        .add_handle_duplicate(
            system_libraries
                .as_ref()
                .unwrap_or(&authorities.library_directory)
                .as_handle_ref(),
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
    match program.authority {
        Some(Authority::Filesystem) => {
            if let Some(control) = authorities.filesystem_control.as_ref() {
                builder
                    .add_handle_duplicate(
                        control.as_handle_ref(),
                        filesystem::CONTROL_CLIENT.as_raw(),
                        RightsOffer::Exact(Rights::WAIT.union(Rights::WRITE)),
                    )
                    .map_err(|_| Error::InvalidCommand)?;
            }
        }
        Some(Authority::Tasks) => builder
            .add_handle_duplicate(
                authorities.task_inspector.as_handle_ref(),
                startup::TASK_INSPECTOR.as_raw(),
                RightsOffer::Exact(Rights::INSPECT),
            )
            .map_err(|_| Error::InvalidCommand)?,
        Some(Authority::Handles) => {
            builder
                .add_handle_duplicate(
                    authorities.object_inspector.as_handle_ref(),
                    startup::OBJECT_INSPECTOR.as_raw(),
                    RightsOffer::Exact(Rights::INSPECT.union(Rights::INSPECT_DETAILS)),
                )
                .map_err(|_| Error::InvalidCommand)?;
            builder
                .add_handle_duplicate(
                    authorities.task_inspector.as_handle_ref(),
                    startup::TASK_INSPECTOR.as_raw(),
                    RightsOffer::Exact(Rights::INSPECT),
                )
                .map_err(|_| Error::InvalidCommand)?;
        }
        Some(Authority::Memory) => builder
            .add_handle_duplicate(
                authorities.memory_inspector.as_handle_ref(),
                startup::MEMORY_INSPECTOR.as_raw(),
                RightsOffer::Exact(Rights::INSPECT),
            )
            .map_err(|_| Error::InvalidCommand)?,
        Some(Authority::Activity) => {
            builder
                .add_handle_duplicate(
                    authorities.task_inspector.as_handle_ref(),
                    startup::TASK_INSPECTOR.as_raw(),
                    RightsOffer::Exact(Rights::INSPECT),
                )
                .map_err(|_| Error::InvalidCommand)?;
            builder
                .add_handle_duplicate(
                    authorities.memory_inspector.as_handle_ref(),
                    startup::MEMORY_INSPECTOR.as_raw(),
                    RightsOffer::Exact(Rights::INSPECT),
                )
                .map_err(|_| Error::InvalidCommand)?;
            builder
                .add_handle_duplicate(
                    authorities.cpu_inspector.as_handle_ref(),
                    startup::CPU_INSPECTOR.as_raw(),
                    RightsOffer::Exact(Rights::INSPECT),
                )
                .map_err(|_| Error::InvalidCommand)?;
        }
        Some(Authority::VirtualMachines) if authorities.vm_connection.is_some() => {
            let (client_control, manager_control) = channel::create_pair().map_err(Error::from)?;
            let (manager_capabilities, client_capabilities) =
                CapabilityChannel::create().map_err(Error::from)?;
            connect_vm_manager(
                authorities
                    .vm_connection
                    .as_ref()
                    .ok_or(Error::InvalidCommand)?,
                manager_control,
                manager_capabilities,
            )?;
            add_child_channel(
                &builder,
                client_control,
                vm::CLIENT_CONTROL.as_raw(),
                vm::CLIENT_CONTROL_CONTRACT.required_rights(),
            )?;
            builder
                .add_handle_move(
                    client_capabilities.into_handle(),
                    vm::CLIENT_CAPABILITIES.as_raw(),
                    RightsOffer::Exact(vm::CLIENT_CAPABILITIES_CONTRACT.required_rights()),
                )
                .map_err(|failure| Error::from(failure.error()))?;
        }
        _ => {}
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
        write_terminal(b"fallback-sh: command failed\r\n")?;
    }
    Ok(())
}

fn connect_vm_manager(
    connector: &CapabilityChannel,
    control: OwnedHandle<ByteChannelObject>,
    capabilities: CapabilityChannel,
) -> Result<(), Error> {
    let mut control = Some(control);
    let mut capabilities = Some(capabilities.into_handle());
    loop {
        let waits = [WaitItem::new(
            connector.as_handle_ref(),
            ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
                .union(ObjectSignals::<CapabilityChannelObject>::PEER_CLOSED),
        )];
        let observation = wait_many(&waits, hyper_os::DEADLINE_INFINITE).map_err(Error::from)?;
        if !ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
            .is_present_in(observation.observed)
        {
            return Err(Error::from(OsError::Status(Status::PEER_CLOSED)));
        }
        let control_disposition = CapabilityDisposition::move_handle(
            &mut control,
            RightsOffer::Exact(Rights::WAIT.union(Rights::READ).union(Rights::WRITE)),
        )
        .map_err(Error::from)?;
        let capabilities_disposition = CapabilityDisposition::move_handle(
            &mut capabilities,
            RightsOffer::Exact(Rights::WAIT.union(Rights::WRITE)),
        )
        .map_err(Error::from)?;
        match connector.try_send(
            &vm::ManagerConnectionRequest.encode(),
            &mut [control_disposition, capabilities_disposition],
        ) {
            Ok(()) => return Ok(()),
            Err(OsError::Status(Status::WOULD_BLOCK)) => {}
            Err(error) => return Err(Error::from(error)),
        }
    }
}

fn add_child_channel(
    builder: &ProcessBuilder,
    channel: OwnedHandle<ByteChannelObject>,
    purpose: u32,
    rights: Rights,
) -> Result<(), Error> {
    let rights = if [
        stdio::STANDARD_INPUT.as_raw(),
        stdio::STANDARD_OUTPUT.as_raw(),
        stdio::STANDARD_ERROR.as_raw(),
    ]
    .contains(&purpose)
    {
        rights.union(Rights::DUPLICATE).union(Rights::TRANSFER)
    } else {
        rights
    };
    builder
        .add_handle_move(channel, purpose, RightsOffer::Exact(rights))
        .map_err(|failure| Error::from(failure.error()))
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
