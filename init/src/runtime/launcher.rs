// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Transactional process construction from a validated launch plan.

use crate::manifest::{LaunchPlan, Manifest, Service};
use hyper_os::handle::{OwnedHandle, ProcessObject, Rights, RightsOffer};
use hyper_os::startup;
use hyper_os::task::{ProcessBuilder, StartFailure};

use super::LaunchError;
use super::authority::AuthorityInventory;
use super::report::report_service_launch_failure;
use super::supervisor::SupervisorSet;

/// Applies validated service declarations to the live authority inventory.
pub(super) struct ServiceLauncher {
    pub(super) authorities: AuthorityInventory,
    pub(super) handoff_enabled: bool,
}

impl ServiceLauncher {
    fn launch_service(
        &mut self,
        service: &Service<'_>,
        service_index: usize,
        plan: &LaunchPlan,
    ) -> Result<OwnedHandle<ProcessObject>, LaunchError> {
        // Each service owns an independent log writer. The raw worker keeps
        // ingress alive across terminal/session restarts.
        self.authorities.service_output_channel = None;
        if service
            .capabilities()
            .any(|grant| grant.source() == "bootstrap.service-output-channel")
        {
            let (writer, reader) =
                hyper_os::channel::create_pair().map_err(LaunchError::service_resources)?;
            crate::contracts::console::register_log(
                self.authorities.log_connector.as_handle_ref(),
                service.name(),
                reader,
            )
            .map_err(LaunchError::service_resources)?;
            self.authorities.service_output_channel = Some(writer);
        }
        let executable = self
            .authorities
            .root_directory
            .open(
                service.image(),
                crate::contracts::process::EXECUTABLE_FILE_RIGHTS,
            )
            .map_err(LaunchError::service_operation)?;
        // Keep the boot scope borrowed independently while capability offers
        // consume other inventory fields. System startup belongs to handoff.
        let environment =
            crate::contracts::process::open_environment(&self.authorities.boot_environment, "/")
                .map_err(LaunchError::service_resources)?;
        let loader = crate::contracts::process::open_loader(
            &environment,
            crate::contracts::process::LoaderKind::Bootstrap,
        )
        .map_err(LaunchError::service_operation)?;
        let builder = ProcessBuilder::create(
            self.authorities.factory.as_handle_ref(),
            self.authorities.group.as_handle_ref(),
            self.authorities.domain.as_handle_ref(),
            executable.as_handle_ref(),
            loader.as_handle_ref(),
        )
        .map_err(LaunchError::service_operation)?;
        builder
            .set_name(service.name())
            .map_err(LaunchError::service_resources)?;
        builder
            .add_argument(service.image())
            .map_err(LaunchError::service_resources)?;
        builder
            .add_handle_duplicate(
                self.authorities.library_directory.as_handle_ref(),
                startup::DYNAMIC_LIBRARY_DIRECTORY.as_raw(),
                RightsOffer::Exact(Rights::READ.union(Rights::EXECUTE).union(Rights::PROTECT)),
            )
            .map_err(LaunchError::service_resources)?;

        for (capability_index, _) in service.capabilities().enumerate() {
            let grant = plan
                .capability_grant(service_index, capability_index)
                .ok_or(LaunchError::InvalidPlan)?;
            self.authorities
                .offer(grant, &builder, &loader, &environment)?;
        }

        // Private lifecycle endpoints are injected by the predecessor, not
        // exposed as service-grant policy or business client connections.
        let purpose = if plan
            .unique_service_for_purpose(startup::DEVICE_ASSIGNMENT_AUTHORITY.as_raw())
            == Some(service_index)
        {
            Some((0x8008_0010, false))
        } else if plan.unique_service_for_purpose(crate::contracts::filesystem::ATTACH.as_raw())
            == Some(service_index)
        {
            Some((0x8008_0011, true))
        } else {
            None
        };
        if let Some((purpose, filesystem)) = purpose.filter(|_| self.handoff_enabled) {
            let (old, successor) = hyper_os::capability_channel::CapabilityChannel::create()
                .map_err(LaunchError::service_resources)?;
            let rights = if filesystem {
                Rights::READ.union(Rights::WRITE).union(Rights::WAIT)
            } else {
                Rights::WRITE.union(Rights::WAIT)
            };
            builder
                .add_handle_move(old.into_handle(), purpose, RightsOffer::Exact(rights))
                .map_err(|failure| LaunchError::service_resources(failure.error()))?;
            if filesystem {
                self.authorities.filesystem_handoff = Some(successor.into_handle());
            } else {
                self.authorities.io_handoff = Some(successor.into_handle());
            }
        }
        if self.handoff_enabled
            && service
                .capabilities()
                .any(|cap| cap.purpose() == "console.system")
        {
            let (old, successor) =
                hyper_os::channel::create_pair().map_err(LaunchError::service_resources)?;
            builder
                .add_handle_move(
                    old,
                    0x8008_0014,
                    RightsOffer::Exact(Rights::READ.union(Rights::WRITE).union(Rights::WAIT)),
                )
                .map_err(|failure| LaunchError::service_resources(failure.error()))?;
            self.authorities.console_drain = Some(successor);
        }
        builder.seal().map_err(LaunchError::service_image)?;
        builder.start().map_err(|failure| {
            let (error, committed) = match failure {
                StartFailure::Rejected { error, .. } => (error, false),
                StartFailure::Committed(error) => (error, true),
            };
            let message = format!(
                "{}service '{}' {} failed: {error:?}\n",
                crate::logging::prefix("init"),
                service.name(),
                if committed { "loader" } else { "start" }
            );
            let _ = self
                .authorities
                .console
                .as_emergency_console()
                .write_all(message.as_bytes());
            if committed {
                LaunchError::service_image(error)
            } else {
                LaunchError::service_resources(error)
            }
        })
    }
}

impl ServiceLauncher {
    pub(super) fn start_services(
        &mut self,
        manifest: &Manifest<'_>,
        plan: &LaunchPlan,
        supervisors: &mut SupervisorSet,
    ) -> Result<(), LaunchError> {
        for position in 0..plan.service_count() {
            let service_index = plan
                .service_index(position)
                .ok_or(LaunchError::InvalidPlan)?;
            let service = manifest
                .service(service_index)
                .ok_or(LaunchError::InvalidPlan)?;
            if supervisors
                .processes
                .get(service_index)
                .ok_or(LaunchError::InvalidPlan)?
                .is_some()
            {
                return Err(LaunchError::InvalidPlan);
            }
            let supervisor = match self.launch_service(service, service_index, plan) {
                Ok(supervisor) => supervisor,
                Err(error) => {
                    report_service_launch_failure(
                        &self.authorities.console,
                        service.name(),
                        &error,
                    );
                    if !service.critical() && matches!(error, LaunchError::ServiceUnavailable) {
                        continue;
                    }
                    return Err(error);
                }
            };
            let slot = supervisors
                .processes
                .get_mut(service_index)
                .ok_or(LaunchError::InvalidPlan)?;
            *slot = Some(supervisor);
        }
        Ok(())
    }
}
