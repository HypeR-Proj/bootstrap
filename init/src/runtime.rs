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

use crate::bootstrap_policy::BootstrapPolicy;
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

const MANIFEST_PATH: &str = "/etc/hyper/services.json";

#[inline(never)]
pub fn run(startup: &mut Startup<'_>) -> Result<(), Error> {
    hyper_os::require_core_abi().map_err(|_| Error::OperatingSystem)?;
    let root_directory = startup
        .take_root_directory()
        .map_err(|_| Error::OperatingSystem)?;
    let source = load_manifest()?;
    let manifest = crate::manifest::parse(&source).map_err(|_| Error::Source)?;
    let plan = crate::manifest::validate(&manifest, &BootstrapPolicy).map_err(|_| Error::Source)?;
    let mut runtime = Runtime::from_startup(startup, root_directory)?;
    let system_init = match root_switch::SystemInitConfig::load() {
        Ok(next) => next,
        Err(_) => {
            let _ = runtime
                .launcher
                .authorities
                .console
                .as_emergency_console()
                .write_all(
                    b"HypeR bootstrap-stage1-init: invalid system-init configuration; retaining recovery services\n",
                );
            None
        }
    };
    runtime
        .launch_validated_graph(&manifest, &plan, system_init.as_ref())
        .map_err(|error| {
            let _ = runtime
                .launcher
                .authorities
                .console
                .as_emergency_console()
                .write_all(error.diagnostic());
            Error::Bootstrap
        })
}

pub enum Error {
    Bootstrap,
    OperatingSystem,
    Source,
}

impl Error {
    pub const fn diagnostic(&self) -> &'static [u8] {
        match self {
            Self::Bootstrap => b"HypeR bootstrap-stage1-init: bootstrap failed\n",
            Self::OperatingSystem => {
                b"HypeR bootstrap-stage1-init: startup capability operation failed\n"
            }
            Self::Source => b"HypeR bootstrap-stage1-init: invalid service manifest source\n",
        }
    }
}

fn load_manifest() -> Result<String, Error> {
    let file = std::fs::File::open(MANIFEST_PATH).map_err(|_| Error::OperatingSystem)?;
    let mut text = String::new();
    file.take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| Error::Source)?;
    if text.len() > MAX_MANIFEST_BYTES {
        return Err(Error::Source);
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
            io_handoff: None,
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
            },
            storage_ready: true,
            root_transition: None,
            boot_libraries: None,
            system_root_mount: 0,
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
        system_init: Option<&root_switch::SystemInitConfig>,
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
                    let _ = self
                        .launcher
                        .authorities
                        .console
                        .as_emergency_console()
                        .write_all(reason.diagnostic());
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
                    authorities.io_handoff.take();
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
            let _ = self.launcher.authorities.console.as_emergency_console().write_all(
                b"HypeR bootstrap-stage1-init: system init unavailable; retaining core supervision and recovery shell\n");
            self.recovery_console(manifest)
        })();
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self
                    .launcher
                    .authorities
                    .console
                    .as_emergency_console()
                    .write_all(error.diagnostic());
                // Recovery owns input only before a committed handoff. Do not
                // tear down healthy storage merely because another service failed.
                self.recovery_console(manifest)
            }
        }
    }

    fn prepare_system_root(
        &mut self,
        next: &root_switch::SystemInitConfig,
    ) -> Result<bool, LaunchError> {
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
                let _ = self
                    .launcher
                    .authorities
                    .console
                    .as_emergency_console()
                    .write_all(
                        b"HypeR bootstrap-stage1-init: provisional system root active; preparing successor\n",
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
            let _ = self
                .launcher
                .authorities
                .console
                .as_emergency_console()
                .write_all(
                b"HypeR bootstrap-stage1-init: system handoff rejected; bootstrap root restored\n",
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

    const fn diagnostic(&self) -> &'static [u8] {
        match self {
            Self::AuthorityConsumed => {
                b"HypeR bootstrap-stage1-init: service authority was already consumed\n"
            }
            Self::CriticalServiceTerminated => {
                b"HypeR bootstrap-stage1-init: critical service terminated\n"
            }
            Self::InvalidPlan => b"HypeR bootstrap-stage1-init: invalid launch plan\n",
            Self::OperatingSystem => {
                b"HypeR bootstrap-stage1-init: service launch operation failed\n"
            }
            Self::RequiredServiceUnavailable => {
                b"HypeR bootstrap-stage1-init: startup provider unavailable\n"
            }
            Self::ServiceUnavailable => {
                b"HypeR bootstrap-stage1-init: service image or resources unavailable\n"
            }
            Self::UnsupportedAuthority => {
                b"HypeR bootstrap-stage1-init: unsupported service authority\n"
            }
            Self::UnsupportedRestartPolicy => {
                b"HypeR bootstrap-stage1-init: unsupported restart policy\n"
            }
            Self::UnsupportedSupervisionGraph => {
                b"HypeR bootstrap-stage1-init: unsupported critical-service supervision graph\n"
            }
            Self::WaitTimedOut => b"HypeR bootstrap-stage1-init: service handshake timed out\n",
        }
    }

    const fn reason(&self) -> &'static [u8] {
        match self {
            Self::AuthorityConsumed => b"authority already consumed",
            Self::CriticalServiceTerminated => b"critical service terminated",
            Self::InvalidPlan => b"invalid launch plan",
            Self::OperatingSystem => b"kernel operation rejected",
            Self::RequiredServiceUnavailable => b"startup provider unavailable",
            Self::ServiceUnavailable => b"service image or resources unavailable",
            Self::UnsupportedAuthority => b"unsupported authority",
            Self::UnsupportedRestartPolicy => b"unsupported restart policy",
            Self::UnsupportedSupervisionGraph => b"unsupported supervision graph",
            Self::WaitTimedOut => b"service handshake timed out",
        }
    }
}
