// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! A failed bootstrap owns the raw recovery shell directly. No formal terminal
//! service or shell decision protocol is needed before the init handoff.

use super::{LaunchError, Runtime};
use crate::contracts::process;
use crate::manifest::Manifest;
use crate::supervision::TerminationAction;
use hyper_os::handle::{OwnedHandle, ProcessObject, Rights, RightsOffer, TaskGroupObject};
use hyper_os::startup;
use hyper_os::task::{ProcessBuilder, create_task_group};
use hyper_os::wait::{ObjectSignals, WaitItem, wait_many};

impl Runtime {
    pub(super) fn recovery_console(&mut self, manifest: &Manifest<'_>) -> Result<(), LaunchError> {
        self.rollback_system_root()?;
        // No successor committed, so providers must stop waiting to offer
        // their state and resume ordinary bootstrap administration. Keeping
        // these peers alive would deadlock recovery mount requests behind an
        // adoption descriptor that nobody will receive.
        self.launcher.authorities.storage_handoff.take();
        self.launcher.authorities.filesystem_handoff.take();
        // The physical worker must have consumed its startup endpoints. If it
        // never launched, binding a recovery stream would wait forever.
        if self.launcher.authorities.raw_control_server.is_some() {
            return Err(LaunchError::RequiredServiceUnavailable);
        }
        let mut epoch = 1_u64;
        loop {
            let group = create_task_group(
                self.launcher.authorities.factory.as_handle_ref(),
                self.launcher.authorities.domain.as_handle_ref(),
            )
            .map_err(LaunchError::service_resources)?;
            let _cleanup = StopOnDrop(&group);
            let child = self.launch_fallback(&group, epoch)?;
            loop {
                let selected = {
                    let mut waits = Vec::new();
                    let mut indices = Vec::new();
                    for (index, process) in self.supervisors.processes.iter().enumerate() {
                        if let Some(process) = process {
                            waits.push(WaitItem::new(
                                process.as_handle_ref(),
                                ObjectSignals::<ProcessObject>::TERMINATED,
                            ));
                            indices.push(index);
                        }
                    }
                    waits.push(WaitItem::new(
                        child.as_handle_ref(),
                        ObjectSignals::<ProcessObject>::TERMINATED,
                    ));
                    let selected = wait_many(&waits, hyper_os::DEADLINE_INFINITE)
                        .map_err(|_| LaunchError::OperatingSystem)?
                        .index;
                    indices.get(selected).copied()
                };
                let Some(index) = selected else {
                    break;
                };
                if self.supervisors.observe_service_termination(
                    manifest,
                    index,
                    None,
                    &self.launcher.authorities.console,
                )? == TerminationAction::FailSystem
                {
                    let _ = group.as_task_group().request_stop();
                    return Err(LaunchError::CriticalServiceTerminated);
                }
            }
            group
                .as_task_group()
                .request_stop()
                .map_err(|_| LaunchError::OperatingSystem)?;
            group
                .as_task_group()
                .wait_terminated(hyper_os::DEADLINE_INFINITE)
                .map_err(|_| LaunchError::OperatingSystem)?;
            epoch = epoch.checked_add(1).ok_or(LaunchError::OperatingSystem)?;
            crate::contracts::console::reset_raw(
                self.launcher
                    .authorities
                    .raw_controller
                    .as_ref()
                    .ok_or(LaunchError::AuthorityConsumed)?
                    .as_handle_ref(),
                epoch,
            )
            .map_err(|_| LaunchError::OperatingSystem)?;
        }
    }

    fn launch_fallback(
        &self,
        group: &OwnedHandle<TaskGroupObject>,
        epoch: u64,
    ) -> Result<OwnedHandle<ProcessObject>, LaunchError> {
        let a = &self.launcher.authorities;
        let streams = crate::contracts::console::attach_raw(
            a.raw_controller
                .as_ref()
                .ok_or(LaunchError::AuthorityConsumed)?
                .as_handle_ref(),
            epoch,
        )
        .map_err(|_| LaunchError::OperatingSystem)?;
        let image = a
            .boot_environment
            .open("svc/fallback-sh", process::EXECUTABLE_FILE_RIGHTS)
            .map_err(LaunchError::service_operation)?;
        let loader = process::open_loader(&a.boot_environment, process::LoaderKind::Bootstrap)
            .map_err(LaunchError::service_operation)?;
        let libraries = process::open_library_directory(&a.boot_environment)
            .map_err(LaunchError::service_operation)?;
        let builder = ProcessBuilder::create(
            a.factory.as_handle_ref(),
            group.as_handle_ref(),
            a.domain.as_handle_ref(),
            image.as_handle_ref(),
            loader.as_handle_ref(),
        )
        .map_err(LaunchError::service_resources)?;
        let configure = || -> hyper_os::Result<()> {
            builder.set_name("fallback-sh")?;
            builder.add_argument("/svc/fallback-sh")?;
            for (handle, purpose, rights) in [
                (
                    streams.input.as_handle_ref(),
                    hyper_os::stream::STANDARD_INPUT.as_raw(),
                    hyper_os::stream::CONNECTOR_RIGHTS,
                ),
                (
                    streams.output.as_handle_ref(),
                    hyper_os::stream::STANDARD_OUTPUT.as_raw(),
                    hyper_os::stream::CONNECTOR_RIGHTS,
                ),
                (
                    streams.output.as_handle_ref(),
                    hyper_os::stream::STANDARD_ERROR.as_raw(),
                    hyper_os::stream::CONNECTOR_RIGHTS,
                ),
            ] {
                builder.add_handle_duplicate(handle, purpose, RightsOffer::Exact(rights))?;
            }
            macro_rules! grant {
                ($h:expr, $c:expr) => {
                    builder.add_handle_duplicate(
                        $h,
                        $c.purpose(),
                        RightsOffer::Exact($c.allowed_rights()),
                    )?;
                };
            }
            grant!(
                a.root_directory.as_handle_ref(),
                process::SHELL_ROOT_DIRECTORY_CONTRACT
            );
            grant!(
                a.factory.as_handle_ref(),
                process::SHELL_TASK_FACTORY_CONTRACT
            );
            grant!(group.as_handle_ref(), process::SHELL_TASK_GROUP_CONTRACT);
            grant!(
                a.domain.as_handle_ref(),
                process::SHELL_RESOURCE_DOMAIN_CONTRACT
            );
            process::ShellDelegation {
                environment: a.boot_environment.as_handle_ref(),
                loader: loader.as_handle_ref(),
                libraries: libraries.as_handle_ref(),
                tasks: a.task_inspector.as_handle_ref(),
                objects: a.object_inspector.as_handle_ref(),
                memory: a.memory_inspector.as_handle_ref(),
                cpus: a.cpu_inspector.as_handle_ref(),
                filesystem: a
                    .filesystem_control_client
                    .as_ref()
                    .map(OwnedHandle::as_handle_ref),
                virtual_machines: None,
            }
            .add_to(&builder)?;
            builder.add_handle_duplicate(
                libraries.as_handle_ref(),
                startup::DYNAMIC_LIBRARY_DIRECTORY.as_raw(),
                RightsOffer::Exact(Rights::READ.union(Rights::EXECUTE).union(Rights::PROTECT)),
            )?;
            builder.seal()
        };
        configure().map_err(LaunchError::service_resources)?;
        builder
            .start()
            .map_err(|failure| LaunchError::service_operation(failure.error()))
    }
}

/// The fallback inherits an attach-only group handle. Dropping init's owner
/// alone cannot stop that group if a wait or launch observation fails.
struct StopOnDrop<'a>(&'a OwnedHandle<TaskGroupObject>);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self.0.as_task_group().request_stop();
    }
}
