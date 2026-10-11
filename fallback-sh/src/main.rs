// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Recovery shell and capability-scoped foreground command launcher.

mod launch;
use launch::launch_command;

use hyper_fallback_sh::command::CommandLine;
use hyper_fallback_sh::editor::{Action, Editor};
use hyper_fallback_sh::path::CanonicalPath;
use hyper_os::Error as OsError;
use hyper_os::capability_channel::CapabilityChannel;
use hyper_os::fs::{Directory, DirectoryRights, File};
use hyper_os::handle::{
    ByteChannelObject, CapabilityChannelObject, CpuInspectorObject, MemoryInspectorObject,
    ObjectInspectorObject, OwnedHandle, ResourceDomainObject, TaskFactoryObject, TaskGroupObject,
    TaskInspectorObject,
};
use hyper_os::startup::{self, Startup};
use hyper_service::{filesystem, process, vm};
use std::io::Write;
use std::process::ExitCode;

const INPUT_CHUNK_BYTES: usize = 256;
const READY_MESSAGE: &[u8] = b"HypeR fallback shell: recovery console ready\n";
const PROMPT: &[u8] = b"fallback-sh$ ";
const WORKING_DIRECTORY_RIGHTS: DirectoryRights = DirectoryRights::READ
    .union(DirectoryRights::WRITE)
    .union(DirectoryRights::SET_ATTRIBUTES)
    .union(DirectoryRights::LOCK_FILE)
    .union(DirectoryRights::INSPECT)
    .union(DirectoryRights::EXECUTE)
    .union(DirectoryRights::PROTECT)
    .union(DirectoryRights::DUPLICATE)
    .union(DirectoryRights::TRANSFER);

fn application_main(mut startup: Startup<'_>) -> ExitCode {
    match run(&mut startup) {
        Ok(code) => code,
        Err(_) => ExitCode::FAILURE,
    }
}

fn run(startup: &mut Startup<'_>) -> Result<ExitCode, Error> {
    hyper_os::require_core_abi().map_err(Error::from)?;
    let input = hyper_rt::process::stdin().map_err(Error::from)?;
    let output = hyper_rt::process::stdout().map_err(Error::from)?;
    // This interactive shell presents one terminal stream. Routing diagnostics
    // through a second session queue lets a prompt overtake pending stderr.
    let error = output;
    let root_directory = startup.take_root_directory().map_err(Error::from)?;
    let environment = Directory::from_handle(
        startup
            .take(process::ENVIRONMENT_DIRECTORY)
            .map_err(Error::from)?,
    );
    // Keep the initial cwd attached to the namespace root. Opening "/" would
    // instead retain the old ramdisk directory across the init root switch.
    let current_directory = root_directory
        .scope_at(&root_directory, WORKING_DIRECTORY_RIGHTS)
        .map_err(Error::from)?;
    let library_directory = Directory::from_handle(
        startup
            .take(process::CHILD_LIBRARY_DIRECTORY)
            .map_err(Error::from)?,
    );
    let loader = File::from_handle(startup.take(process::CHILD_LOADER).map_err(Error::from)?);
    let factory = startup.take(startup::TASK_FACTORY).map_err(Error::from)?;
    let group = startup.take(startup::TASK_GROUP).map_err(Error::from)?;
    let domain = startup
        .take(startup::RESOURCE_DOMAIN)
        .map_err(Error::from)?;
    let task_inspector = startup.take(startup::TASK_INSPECTOR).map_err(Error::from)?;
    let object_inspector = startup
        .take(startup::OBJECT_INSPECTOR)
        .map_err(Error::from)?;
    let memory_inspector = startup
        .take(startup::MEMORY_INSPECTOR)
        .map_err(Error::from)?;
    let cpu_inspector = startup.take(startup::CPU_INSPECTOR).map_err(Error::from)?;
    let vm_connection = startup
        .take_optional(vm::MANAGER_CONNECTION)
        .map_err(Error::from)?
        .map(CapabilityChannel::from_handle);
    let filesystem_control = startup
        .take_optional(filesystem::CONTROL_CLIENT)
        .map_err(Error::from)?;
    let mut authorities = CommandAuthorities {
        root_directory,
        environment,
        current_directory,
        current_path: CanonicalPath::root(),
        library_directory,
        loader,
        factory,
        group,
        domain,
        task_inspector,
        object_inspector,
        memory_inspector,
        cpu_inspector,
        vm_connection,
        filesystem_control,
    };

    write_terminal(READY_MESSAGE)?;
    write_terminal(PROMPT)?;

    let mut editor = Editor::default();
    let mut input_chunk = [0_u8; INPUT_CHUNK_BYTES];
    loop {
        let count = input
            .as_byte_channel()
            .receive(&mut input_chunk)
            .map_err(Error::from)?;
        let bytes = input_chunk.get(..count).ok_or(Error::Protocol)?;
        for byte in bytes.iter().copied() {
            match editor.push(byte) {
                Action::None => (),
                Action::Echo(byte) => write_terminal(&[byte])?,
                Action::Redraw | Action::ClearScreen => {
                    // Redraw the whole line so multibyte deletion does not
                    // leave stale text behind the cursor.
                    if byte == 12 {
                        write_terminal(b"\x1b[2J\x1b[H")?;
                    }
                    write_terminal(b"\r\x1b[2K")?;
                    write_terminal(PROMPT)?;
                    write_terminal(editor.line())?;
                }
                Action::Submit(line) => {
                    write_terminal(b"\n")?;
                    match execute_line(&line, &mut authorities, input, output, error) {
                        Ok(CommandFlow::Continue) => (),
                        Ok(CommandFlow::Exit) => return Ok(ExitCode::SUCCESS),
                        Err(error) => write_terminal(format!("fallback-sh: {error}\n").as_bytes())?,
                    }
                    write_terminal(PROMPT)?;
                }
                Action::TooLong => {
                    write_terminal(b"\nsh: command line is too long\n")?;
                    write_terminal(PROMPT)?;
                }
                Action::Cancel => {
                    write_terminal(b"^C\n")?;
                    write_terminal(PROMPT)?;
                }
                Action::Exit => return Ok(ExitCode::SUCCESS),
            }
        }
    }
}

fn execute_line(
    bytes: &[u8],
    authorities: &mut CommandAuthorities,
    input: &OwnedHandle<ByteChannelObject>,
    output: &OwnedHandle<ByteChannelObject>,
    error: &OwnedHandle<ByteChannelObject>,
) -> Result<CommandFlow, Error> {
    let command = match CommandLine::parse(bytes) {
        Ok(command) => command,
        Err(_) => {
            write_terminal(b"fallback-sh: invalid command syntax\n")?;
            return Ok(CommandFlow::Continue);
        }
    };
    let Some(name) = command.argument(0) else {
        return Ok(CommandFlow::Continue);
    };
    let arguments: Vec<_> = command.arguments().collect();
    match arguments.as_slice() {
        ["help"] => write_terminal(b"Recovery shell: cd [DIRECTORY], pwd, help, exit\nRun mount/umount or an explicit executable path. Quoted arguments are supported; pipelines and redirection are not.\n")?,
        ["cd"] => builtin_cd("/", authorities)?,
        ["cd", target] => builtin_cd(target, authorities)?,
        ["pwd"] => writeln!(std::io::stdout(), "{}", authorities.current_path.as_str().map_err(|_| Error::InvalidCommand)?).map_err(Error::Io)?,
        ["exit"] => return Ok(CommandFlow::Exit),
        _ if matches!(name, "cd" | "pwd" | "help" | "exit") => write_terminal(b"fallback-sh: invalid builtin arguments\n")?,
        _ => launch_command(&command, bytes, authorities, input, output, error)?,
    }
    Ok(CommandFlow::Continue)
}

fn builtin_cd(target: &str, authorities: &mut CommandAuthorities) -> Result<(), Error> {
    let path = match authorities.current_path.resolve(target) {
        Ok(path) => path,
        Err(_) => {
            write_terminal(b"cd: invalid directory path\n")?;
            return Ok(());
        }
    };
    let path_text = path.as_str().map_err(|_| Error::InvalidCommand)?;
    let directory = if path_text == "/" {
        authorities
            .root_directory
            .scope_at(&authorities.root_directory, WORKING_DIRECTORY_RIGHTS)
    } else {
        authorities
            .root_directory
            .open_directory(path_text, WORKING_DIRECTORY_RIGHTS)
    };
    let directory = match directory {
        Ok(directory) => directory,
        Err(_) => {
            write_terminal(b"cd: cannot open directory\n")?;
            return Ok(());
        }
    };
    authorities.current_directory = directory;
    authorities.current_path = path;
    Ok(())
}

fn write_terminal(bytes: &[u8]) -> Result<(), Error> {
    let mut output = std::io::stdout().lock();
    output.write_all(bytes).map_err(Error::Io)?;
    // Flush prompts and individual key echoes before the next blocking wait.
    output.flush().map_err(Error::Io)
}

struct CommandAuthorities {
    root_directory: Directory,
    environment: Directory,
    current_directory: Directory,
    current_path: CanonicalPath,
    library_directory: Directory,
    loader: File,
    factory: OwnedHandle<TaskFactoryObject>,
    group: OwnedHandle<TaskGroupObject>,
    domain: OwnedHandle<ResourceDomainObject>,
    task_inspector: OwnedHandle<TaskInspectorObject>,
    object_inspector: OwnedHandle<ObjectInspectorObject>,
    memory_inspector: OwnedHandle<MemoryInspectorObject>,
    cpu_inspector: OwnedHandle<CpuInspectorObject>,
    vm_connection: Option<CapabilityChannel>,
    filesystem_control: Option<OwnedHandle<CapabilityChannelObject>>,
}

enum CommandFlow {
    Continue,
    Exit,
}

#[derive(Debug)]
enum Error {
    OperatingSystem(OsError),
    Io(std::io::Error),
    InvalidCommand,
    Protocol,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OperatingSystem(error) => write!(formatter, "Native operation failed: {error:?}"),
            Self::Io(error) => write!(formatter, "I/O failed: {error}"),
            Self::InvalidCommand => formatter.write_str("command launch failed"),
            Self::Protocol => formatter.write_str("protocol violation"),
        }
    }
}

impl From<OsError> for Error {
    fn from(error: OsError) -> Self {
        Self::OperatingSystem(error)
    }
}

fn main() -> ExitCode {
    match hyper_rt::process::startup() {
        Ok(startup) => application_main(startup),
        Err(_) => ExitCode::FAILURE,
    }
}
