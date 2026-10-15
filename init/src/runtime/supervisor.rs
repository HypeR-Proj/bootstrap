// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Service process supervision during startup handshakes and steady operation.

use crate::manifest::{MAX_SERVICES, Manifest};
use crate::supervision::{self, TerminationAction};
use hyper_os::handle::{ByteChannelObject, ConsoleObject, OwnedHandle, ProcessObject};
use hyper_os::wait::{ObjectSignals, WaitItem, wait_many};

use super::LaunchError;
use super::report::report_service_termination;

const _: () = assert!(MAX_SERVICES < hyper_os::wait::MAX_ITEMS);

pub(super) enum StorageReadiness {
    Ready,
    Unavailable(StorageFailure),
}

pub(super) enum StorageFailure {
    ProviderUnavailable,
    ChannelClosed,
    InvalidReply,
    MountUnavailable,
    TimedOut,
}

impl StorageFailure {
    pub(super) const fn diagnostic(&self) -> &'static str {
        match self {
            Self::ProviderUnavailable => "storage unavailable: provider unavailable\n",
            Self::ChannelClosed => "storage unavailable: readiness channel closed\n",
            Self::InvalidReply => "storage unavailable: invalid readiness reply\n",
            Self::MountUnavailable => "storage unavailable: required mount failed\n",
            Self::TimedOut => "storage unavailable: readiness timed out\n",
        }
    }
}

/// Stable service-index ownership and wait-set mapping.
pub(super) struct SupervisorSet {
    pub(super) processes: [Option<OwnedHandle<ProcessObject>>; MAX_SERVICES],
}

impl SupervisorSet {
    /// Waits for one startup endpoint while continuing to supervise every
    /// service. The endpoint's provider must remain alive until completion.
    pub(super) fn wait_for_service_event(
        &mut self,
        manifest: &Manifest<'_>,
        required_service: usize,
        event: WaitItem<'_>,
        deadline: u64,
        console: &OwnedHandle<ConsoleObject>,
    ) -> Result<u64, LaunchError> {
        let provider = manifest
            .service(required_service)
            .ok_or(LaunchError::InvalidPlan)?;
        if self
            .processes
            .get(required_service)
            .and_then(Option::as_ref)
            .is_none()
        {
            return Err(if provider.critical() {
                LaunchError::CriticalServiceTerminated
            } else {
                LaunchError::RequiredServiceUnavailable
            });
        }
        loop {
            let selected = {
                let mut items = Vec::with_capacity(MAX_SERVICES + 1);
                let mut services = Vec::with_capacity(MAX_SERVICES);
                // Critical exits must win even when an optional provider exits
                // or sends its readiness result in the same observation.
                for critical in [true, false] {
                    for (index, process) in self.processes.iter().enumerate() {
                        if let Some(process) = process {
                            let service =
                                manifest.service(index).ok_or(LaunchError::InvalidPlan)?;
                            if service.critical() != critical {
                                continue;
                            }
                            items.push(WaitItem::new(
                                process.as_handle_ref(),
                                ObjectSignals::<ProcessObject>::TERMINATED,
                            ));
                            services.push(index);
                        }
                    }
                }
                // Process events precede endpoint readiness, so a queued
                // successful reply cannot hide an observed critical failure.
                items.push(event);
                let observation = wait_many(&items, deadline).map_err(|error| match error {
                    hyper_os::Error::Status(hyper_os::Status::TIMED_OUT) => {
                        LaunchError::WaitTimedOut
                    }
                    _ => LaunchError::OperatingSystem,
                })?;
                if observation.index == services.len() {
                    return Ok(observation.observed);
                }
                *services
                    .get(observation.index)
                    .ok_or(LaunchError::InvalidPlan)?
            };
            match self.observe_service_termination(
                manifest,
                selected,
                Some(required_service),
                console,
            )? {
                TerminationAction::Continue => {}
                TerminationAction::ProviderUnavailable => {
                    return Err(LaunchError::RequiredServiceUnavailable);
                }
                TerminationAction::FailSystem => {
                    return Err(LaunchError::CriticalServiceTerminated);
                }
            }
        }
    }

    pub(super) fn wait_for_storage(
        &mut self,
        manifest: &Manifest<'_>,
        ready_service: usize,
        reader: &OwnedHandle<ByteChannelObject>,
        console: &OwnedHandle<ConsoleObject>,
    ) -> Result<StorageReadiness, LaunchError> {
        let deadline = hyper_os::time::deadline_after(std::time::Duration::from_secs(
            crate::contracts::io::READY_TIMEOUT_SECONDS,
        ))
        .map_err(|_| LaunchError::OperatingSystem)?
        .as_raw();
        loop {
            let observed = match self.wait_for_service_event(
                manifest,
                ready_service,
                WaitItem::new(
                    reader.as_handle_ref(),
                    ObjectSignals::<ByteChannelObject>::READABLE
                        .union(ObjectSignals::<ByteChannelObject>::PEER_CLOSED),
                ),
                deadline,
                console,
            ) {
                Ok(observed) => observed,
                Err(LaunchError::WaitTimedOut) => {
                    return Ok(StorageReadiness::Unavailable(StorageFailure::TimedOut));
                }
                Err(LaunchError::RequiredServiceUnavailable) => {
                    return Ok(StorageReadiness::Unavailable(
                        StorageFailure::ProviderUnavailable,
                    ));
                }
                Err(error) => return Err(error),
            };
            if !ObjectSignals::<ByteChannelObject>::READABLE.is_present_in(observed) {
                return Ok(StorageReadiness::Unavailable(StorageFailure::ChannelClosed));
            }
            let mut message = [0; 64];
            match reader.as_byte_channel().try_receive(&mut message) {
                Ok(length)
                    if message.get(..length) == Some(crate::contracts::io::READY_MESSAGE) =>
                {
                    return Ok(StorageReadiness::Ready);
                }
                Ok(length)
                    if message.get(..length)
                        == Some(crate::contracts::filesystem::FAILURE_MESSAGE) =>
                {
                    return Ok(StorageReadiness::Unavailable(
                        StorageFailure::MountUnavailable,
                    ));
                }
                Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => continue,
                Err(hyper_os::Error::Status(hyper_os::Status::PEER_CLOSED)) => {
                    return Ok(StorageReadiness::Unavailable(StorageFailure::ChannelClosed));
                }
                Err(hyper_os::Error::MessageTooLarge { .. }) => {
                    return Ok(StorageReadiness::Unavailable(StorageFailure::InvalidReply));
                }
                Ok(_) => return Ok(StorageReadiness::Unavailable(StorageFailure::InvalidReply)),
                Err(_) => return Err(LaunchError::OperatingSystem),
            }
        }
    }

    pub(super) fn observe_service_termination(
        &mut self,
        manifest: &Manifest<'_>,
        index: usize,
        required_service: Option<usize>,
        console: &OwnedHandle<ConsoleObject>,
    ) -> Result<TerminationAction, LaunchError> {
        let service = manifest.service(index).ok_or(LaunchError::InvalidPlan)?;
        let process = self
            .processes
            .get_mut(index)
            .and_then(Option::take)
            .ok_or(LaunchError::InvalidPlan)?;
        report_service_termination(console, service.name(), service.critical(), &process);
        Ok(supervision::startup_termination_action(
            service.critical(),
            required_service == Some(index),
        ))
    }
}
