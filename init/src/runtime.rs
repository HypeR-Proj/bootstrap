// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Capability-scoped adapter from a validated manifest to `ProcessBuilder`.

mod authority;
mod handoff;
mod launcher;
mod recovery;
mod report;
mod root_switch;
mod supervisor;

use authority::AuthorityInventory;
use launcher::ServiceLauncher;
use supervisor::{StorageReadiness, SupervisorSet};

use std::io::Read;

use crate::manifest::{LaunchPlan, MAX_MANIFEST_BYTES, Manifest};
use crate::supervision::{self, SupportError};
use hyper_os::capability_channel::CapabilityChannel;
use hyper_os::channel;
use hyper_os::fs::Directory;
use hyper_os::startup::{self, Startup};

#[inline(never)]
pub fn run(startup: &mut Startup<'_>) -> Result<(), Error> {
    hyper_os::require_core_abi().map_err(|_| Error::OperatingSystem)?;
    let root_directory = startup
        .take_root_directory()
        .map_err(|_| Error::OperatingSystem)?;
    let command_line = startup
        .take_boot_command_line()
        .map_err(|error| Error::source("boot command line", format_args!("{error:?}")))?;
    let selected = crate::profile::select(command_line.as_deref().unwrap_or(""))
        .map_err(|error| Error::source("boot profile selector", error))?;
    let profile_path = format!("{}/{selected}.json", crate::profile::DIRECTORY);
    let profile =
        crate::profile::Profile::parse(&load_text(&profile_path, crate::profile::MAX_BYTES)?)
            .map_err(|error| Error::source(&profile_path, error))?;
    let source = load_text(&profile.services, MAX_MANIFEST_BYTES)?;
    let manifest = crate::manifest::parse(&source)
        .map_err(|error| Error::source(&profile.services, format_args!("{error:?}")))?;
    let plan = profile
        .validate(&manifest)
        .map_err(|error| Error::source(&profile_path, error))?;
    let mut runtime = Runtime::from_startup(startup, root_directory)?;
    runtime.launcher.storage_service = profile
        .storage_service(&manifest)
        .map_err(|error| Error::source(&profile_path, error))?;
    runtime.storage_successor = profile.io.successor().map(str::to_owned);
    report::write(
        &runtime.launcher.authorities.console,
        format_args!("boot profile: {selected}\n"),
    );
    runtime
        .launch_validated_graph(&manifest, &plan, profile.handoff.as_ref())
        .map_err(|error| {
            report::write(
                &runtime.launcher.authorities.console,
                format_args!("{}", error.diagnostic()),
            );
            Error::Bootstrap
        })
}

pub enum Error {
    Bootstrap,
    OperatingSystem,
    Source(String),
}

impl Error {
    pub fn diagnostic(&self) -> &str {
        match self {
            Self::Bootstrap => "bootstrap failed\n",
            Self::OperatingSystem => "startup capability operation failed\n",
            Self::Source(message) => message,
        }
    }

    fn source(context: &str, reason: impl std::fmt::Display) -> Self {
        Self::Source(format!("{context}: {reason}\n"))
    }
}

fn load_text(path: &str, maximum: usize) -> Result<String, Error> {
    let file = std::fs::File::open(path).map_err(|error| Error::source(path, error))?;
    let mut text = String::new();
    file.take(maximum as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|error| Error::source(path, error))?;
    if text.len() > maximum {
        return Err(Error::source(path, "configuration exceeds size limit"));
    }
    Ok(text)
}

/// Top-level init state. Subobjects own disjoint authority and lifecycle roles.
struct Runtime {
    launcher: ServiceLauncher,
    supervisors: SupervisorSet,
    storage_ready: bool,
    root_transition: Option<hyper_os::OwnedHandle<hyper_os::handle::RootTransitionObject>>,
    boot_libraries: Option<Directory>,
    system_root_mount: u64,
    storage_successor: Option<String>,
}

impl Runtime {
    fn from_startup(startup: &mut Startup<'_>, root_directory: Directory) -> Result<Self, Error> {
        let (raw_control_server, raw_controller) =
            CapabilityChannel::create().map_err(|_| Error::OperatingSystem)?;
        let (log_server, log_connector) =
            CapabilityChannel::create().map_err(|_| Error::OperatingSystem)?;
        let factory = startup
            .take(startup::TASK_FACTORY)
            .map_err(|_| Error::OperatingSystem)?;
        let group = startup
            .take(startup::TASK_GROUP)
            .map_err(|_| Error::OperatingSystem)?;
        let domain = startup
            .take(startup::RESOURCE_DOMAIN)
            .map_err(|_| Error::OperatingSystem)?;
        let boot_environment = crate::contracts::process::open_environment(&root_directory, "/")
            .map_err(|_| Error::OperatingSystem)?;
        let library_directory =
            crate::contracts::process::open_library_directory(&boot_environment)
                .map_err(|_| Error::OperatingSystem)?;
        let authorities = AuthorityInventory {
            root_directory,
            boot_environment,
            system_environment: None,
            library_directory,
            factory,
            group,
            domain,
            vm_creation: startup
                .take_optional(startup::VIRTUAL_MACHINE_CREATION_AUTHORITY)
                .map_err(|_| Error::OperatingSystem)?,
            device: startup
                .take_optional(startup::DEVICE_ASSIGNMENT_AUTHORITY)
                .map_err(|_| Error::OperatingSystem)?,
            task_inspector: startup
                .take(startup::TASK_INSPECTOR)
                .map_err(|_| Error::OperatingSystem)?,
            object_inspector: startup
                .take(startup::OBJECT_INSPECTOR)
                .map_err(|_| Error::OperatingSystem)?,
            memory_inspector: startup
                .take(startup::MEMORY_INSPECTOR)
                .map_err(|_| Error::OperatingSystem)?,
            cpu_inspector: startup
                .take(startup::CPU_INSPECTOR)
                .map_err(|_| Error::OperatingSystem)?,
            console: {
                let console = hyper_rt::process::console().map_err(|_| Error::OperatingSystem)?;
                console
                    .duplicate(console.info().map_err(|_| Error::OperatingSystem)?.rights)
                    .map_err(|_| Error::OperatingSystem)?
            },
            raw_control_server: Some(raw_control_server.into_handle()),
            raw_controller: Some(raw_controller.into_handle()),
            log_server: Some(log_server.into_handle()),
            log_connector: log_connector.into_handle(),
            service_output_channel: None,
            io_ready_channel: None,
            console_drain: None,
            storage_handoff: None,
            filesystem_handoff: None,
            io_broker_server: None,
            io_broker_client: None,
            filesystem_attach: None,
            filesystem_provider: None,
            filesystem_control_server: None,
            filesystem_control_client: None,
        };
        Ok(Self {
            launcher: ServiceLauncher {
                authorities,
                handoff_enabled: false,
                storage_service: None,
            },
            storage_ready: true,
            root_transition: None,
            boot_libraries: None,
            system_root_mount: 0,
            storage_successor: None,
            supervisors: SupervisorSet {
                processes: std::array::from_fn(|_| None),
            },
        })
    }

    fn preflight(manifest: &Manifest<'_>) -> Result<(), LaunchError> {
        match supervision::validate(manifest) {
            Ok(()) => Ok(()),
            Err(SupportError::RestartPolicy) => Err(LaunchError::UnsupportedRestartPolicy),
            Err(SupportError::MissingCriticalService) => {
                Err(LaunchError::UnsupportedSupervisionGraph)
            }
        }
    }
}

impl Runtime {
    /// Starts core services and offers supervision to the successor.
    ///
    /// A failed mount keeps the physical backend and filesystem manager alive
    /// for recovery. Only committed handoff ends boot supervision.
    /// Failures after service launch request service shutdown before return.
    #[inline(never)]
    fn launch_validated_graph(
        &mut self,
        manifest: &Manifest<'_>,
        plan: &LaunchPlan,
        system_init: Option<&crate::profile::Handoff>,
    ) -> Result<(), LaunchError> {
        Self::preflight(manifest)?;
        self.launcher.handoff_enabled = system_init.is_some();
        {
            if crate::bootstrap_policy::filesystem_enabled(plan)
                .map_err(|_| LaunchError::InvalidPlan)?
            {
                let (manager, provider) =
                    CapabilityChannel::create().map_err(|_| LaunchError::OperatingSystem)?;
                self.launcher.authorities.filesystem_attach = Some(manager.into_handle());
                self.launcher.authorities.filesystem_provider = Some(provider.into_handle());
            }
            if plan
                .unique_service_for_purpose(crate::contracts::io::BROKER_SERVER.as_raw())
                .is_some()
            {
                let (server, client) =
                    CapabilityChannel::create().map_err(|_| LaunchError::OperatingSystem)?;
                self.launcher.authorities.io_broker_server = Some(server.into_handle());
                self.launcher.authorities.io_broker_client = Some(client.into_handle());
            }
            if plan
                .unique_service_for_purpose(crate::contracts::filesystem::CONTROL_SERVER.as_raw())
                .is_some()
            {
                let (server, client) =
                    CapabilityChannel::create().map_err(|_| LaunchError::OperatingSystem)?;
                self.launcher.authorities.filesystem_control_server = Some(server.into_handle());
                self.launcher.authorities.filesystem_control_client = Some(client.into_handle());
            }
        }
        let ready_service = crate::bootstrap_policy::io_ready_service(plan)
            .map_err(|_| LaunchError::InvalidPlan)?;
        let ready_reader = if ready_service.is_some() {
            let (writer, reader) =
                channel::create_pair().map_err(|_| LaunchError::OperatingSystem)?;
            self.launcher.authorities.io_ready_channel = Some(writer);
            Some(reader)
        } else {
            None
        };
        let result = (|| {
            {
                self.launcher
                    .start_services(manifest, plan, &mut self.supervisors)?;
            }
            let mut storage_available = true;
            if let (Some(reader), Some(service)) = (ready_reader.as_ref(), ready_service) {
                let readiness = self.supervisors.wait_for_storage(
                    manifest,
                    service,
                    reader,
                    &self.launcher.authorities.console,
                )?;
                if let StorageReadiness::Unavailable(reason) = readiness {
                    report::write(
                        &self.launcher.authorities.console,
                        format_args!("{}", reason.diagnostic()),
                    );
                    storage_available = false;
                }
            }
            drop(ready_reader);
            self.storage_ready = storage_available;
            if let Some(next) = system_init {
                // Only bootstrap mounts precede handoff. System-init availability
                // is decided by its executable and library capabilities.
                if self.prepare_system_root(next)? && handoff::transfer(self, manifest, next) {
                    let authorities = &mut self.launcher.authorities;
                    authorities.service_output_channel.take();
                    // The successor is now the only migration coordinator.
                    // Retained endpoints would hide its death from providers;
                    // retained log writers would prevent the final raw drain.
                    authorities.storage_handoff.take();
                    authorities.filesystem_handoff.take();
                    authorities.console_drain.take();
                    // The successful rendezvous moved all process owners.
                    // System init owns retirement of this group. Keep the
                    // remaining leases alive without a second policy loop.
                    loop {
                        std::thread::park();
                    }
                }
                self.rollback_system_root()?;
            }
            self.launcher.authorities.io_broker_client.take();
            report::write(
                &self.launcher.authorities.console,
                format_args!(
                    "system init unavailable; retaining core supervision and recovery shell\n"
                ),
            );
            self.recovery_console(manifest)
        })();
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                report::write(
                    &self.launcher.authorities.console,
                    format_args!("{}", error.diagnostic()),
                );
                // Recovery owns input only before a committed handoff. Do not
                // tear down healthy storage merely because another service failed.
                self.recovery_console(manifest)
            }
        }
    }

    fn prepare_system_root(&mut self, next: &crate::profile::Handoff) -> Result<bool, LaunchError> {
        let prepared = (|| {
            let environment = crate::contracts::process::open_environment(
                &self.launcher.authorities.root_directory,
                &next.system_root,
            )?;
            let libraries = crate::contracts::process::open_library_directory(&environment)?;
            let (transition, mount) = next.enter(&self.launcher.authorities.root_directory)?;
            Ok((environment, libraries, transition, mount))
        })();
        match prepared {
            Ok((environment, libraries, transition, mount)) => {
                self.launcher.authorities.system_environment = Some(environment);
                self.boot_libraries = Some(std::mem::replace(
                    &mut self.launcher.authorities.library_directory,
                    libraries,
                ));
                self.root_transition = Some(transition);
                self.system_root_mount = mount;
                report::write(
                    &self.launcher.authorities.console,
                    format_args!("provisional system root active; preparing successor\n"),
                );
                Ok(true)
            }
            Err(error) if supervision::service_unavailable(&error) => {
                report::report_operation_failure(
                    &self.launcher.authorities.console,
                    "cannot prepare system root",
                    &error,
                );
                Ok(false)
            }
            Err(_) => Err(LaunchError::OperatingSystem),
        }
    }

    fn rollback_system_root(&mut self) -> Result<(), LaunchError> {
        if let Some(transition) = self.root_transition.take() {
            hyper_os::filesystem::finish_root_switch(transition.as_handle_ref(), false)
                .map_err(|_| LaunchError::OperatingSystem)?;
            self.launcher.authorities.system_environment.take();
            self.launcher.authorities.library_directory = self
                .boot_libraries
                .take()
                .ok_or(LaunchError::AuthorityConsumed)?;
            report::write(
                &self.launcher.authorities.console,
                format_args!("system handoff rejected; bootstrap root restored\n"),
            );
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) enum LaunchError {
    AuthorityConsumed,
    CriticalServiceTerminated,
    InvalidPlan,
    OperatingSystem,
    RequiredServiceUnavailable,
    ServiceUnavailable,
    UnsupportedAuthority,
    UnsupportedRestartPolicy,
    UnsupportedSupervisionGraph,
    WaitTimedOut,
}

impl LaunchError {
    fn service_operation(error: hyper_os::Error) -> Self {
        if supervision::service_unavailable(&error) {
            Self::ServiceUnavailable
        } else {
            Self::OperatingSystem
        }
    }

    fn service_image(error: hyper_os::Error) -> Self {
        if supervision::service_image_unavailable(&error) {
            Self::ServiceUnavailable
        } else {
            Self::OperatingSystem
        }
    }

    fn service_resources(error: hyper_os::Error) -> Self {
        if supervision::service_resources_unavailable(&error) {
            Self::ServiceUnavailable
        } else {
            Self::OperatingSystem
        }
    }

    const fn diagnostic(&self) -> &'static str {
        match self {
            Self::AuthorityConsumed => "service authority was already consumed\n",
            Self::CriticalServiceTerminated => "critical service terminated\n",
            Self::InvalidPlan => "invalid launch plan\n",
            Self::OperatingSystem => "service launch operation failed\n",
            Self::RequiredServiceUnavailable => "startup provider unavailable\n",
            Self::ServiceUnavailable => "service image or resources unavailable\n",
            Self::UnsupportedAuthority => "unsupported service authority\n",
            Self::UnsupportedRestartPolicy => "unsupported restart policy\n",
            Self::UnsupportedSupervisionGraph => "unsupported critical-service supervision graph\n",
            Self::WaitTimedOut => "service handshake timed out\n",
        }
    }

    const fn reason(&self) -> &'static str {
        match self {
            Self::AuthorityConsumed => "authority already consumed",
            Self::CriticalServiceTerminated => "critical service terminated",
            Self::InvalidPlan => "invalid launch plan",
            Self::OperatingSystem => "kernel operation rejected",
            Self::RequiredServiceUnavailable => "startup provider unavailable",
            Self::ServiceUnavailable => "service image or resources unavailable",
            Self::UnsupportedAuthority => "unsupported authority",
            Self::UnsupportedRestartPolicy => "unsupported restart policy",
            Self::UnsupportedSupervisionGraph => "unsupported supervision graph",
            Self::WaitTimedOut => "service handshake timed out",
        }
    }
}
