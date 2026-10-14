// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! A capability rendezvous is the only supervisor-ownership commit point.
//! The candidate parses its complete service graph before registering receive.
//! Failure before commit leaves every Process owner in the boot supervisor.

mod inventory;
mod metadata;

use std::time::Duration;

use hyper_os::capability_channel::{CapabilityChannel, CapabilityDisposition};
use hyper_os::handle::{
    CapabilityChannelObject, OwnedHandle, ProcessObject, Rights, RightsOffer, TaskGroupObject,
};
use hyper_os::startup;
use hyper_os::task::{ProcessBuilder, create_task_group};
use hyper_os::wait::{ObjectSignals, WaitItem, wait_many};

use super::{Runtime, root_switch::SystemInitConfig};
use crate::manifest::{MAX_SERVICES, Manifest};
use inventory::InventoryOffer;
use metadata::PreparedOffer;

const COMMIT: &[u8] = b"HYPER-INIT-HANDOFF/3";
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(30);
const ENDPOINT: startup::StartupPurpose<CapabilityChannelObject> =
    startup::StartupPurpose::new(0x8008_0001);
const _: () = assert!(MAX_SERVICES + 2 <= hyper_os::wait::MAX_ITEMS);

/// Starts a candidate with no core supervisor or management authority. Only
/// the final rendezvous grants its validated inventory. Duplicate group leases
/// overlap until boot exits, so no zero-active-handles stop can race the move.
pub(super) fn transfer(
    runtime: &mut Runtime,
    manifest: &Manifest<'_>,
    next: &SystemInitConfig,
) -> bool {
    match try_transfer(runtime, manifest, next) {
        Ok(committed) => committed,
        Err(error) => {
            super::report::report_operation_failure(
                &runtime.launcher.authorities.console,
                "system init handoff rejected",
                &error,
            );
            false
        }
    }
}

fn try_transfer(
    runtime: &mut Runtime,
    manifest: &Manifest<'_>,
    next: &SystemInitConfig,
) -> hyper_os::Result<bool> {
    let a = &runtime.launcher.authorities;
    let candidate_group = create_task_group(a.factory.as_handle_ref(), a.domain.as_handle_ref())?;
    let offer = PreparedOffer::new(runtime, &candidate_group, manifest, &next.put_old)?;
    let snapshot = offer.snapshot()?;
    let (sender, receiver) = CapabilityChannel::create()?;
    let executable = a.root_directory.open(
        &next.system_init,
        crate::contracts::process::EXECUTABLE_FILE_RIGHTS,
    )?;
    let loader = crate::contracts::process::open_loader(
        a.system_environment
            .as_ref()
            .ok_or(hyper_os::Error::MissingHandle)?,
        crate::contracts::process::LoaderKind::Dynamic,
    )?;
    let builder = ProcessBuilder::create(
        a.factory.as_handle_ref(),
        candidate_group.as_handle_ref(),
        a.domain.as_handle_ref(),
        executable.as_handle_ref(),
        loader.as_handle_ref(),
    )?;
    builder.set_name("init")?;
    builder.add_argument(&next.system_init)?;
    builder.set_memory_lock_allowed(true)?;
    builder.add_handle_duplicate(
        a.root_directory.as_handle_ref(),
        startup::ROOT_DIRECTORY.as_raw(),
        // libhyper retains a root by duplicating it, and std file opens request
        // INSPECT on the derived file. Preparation needs no filesystem writes.
        RightsOffer::Exact(
            Rights::READ
                .union(Rights::EXECUTE)
                .union(Rights::INSPECT)
                .union(Rights::DUPLICATE),
        ),
    )?;
    builder.add_handle_duplicate(
        a.library_directory.as_handle_ref(),
        startup::DYNAMIC_LIBRARY_DIRECTORY.as_raw(),
        RightsOffer::Exact(Rights::READ.union(Rights::EXECUTE).union(Rights::PROTECT)),
    )?;
    builder.add_handle_duplicate(
        a.console.as_handle_ref(),
        startup::CONSOLE.as_raw(),
        RightsOffer::Exact(
            Rights::WRITE
                .union(Rights::WAIT)
                .union(Rights::DUPLICATE)
                .union(Rights::TRANSFER),
        ),
    )?;
    builder.add_handle_duplicate(
        snapshot.as_handle_ref(),
        metadata::SNAPSHOT.as_raw(),
        RightsOffer::Exact(Rights::READ.union(Rights::INSPECT)),
    )?;
    builder
        .add_handle_move(
            receiver.into_handle(),
            ENDPOINT.as_raw(),
            RightsOffer::Exact(Rights::WAIT.union(Rights::READ)),
        )
        .map_err(|failure| failure.into_parts().0)?;
    builder.seal()?;
    let process = builder.start().map_err(|failure| match failure {
        hyper_os::task::StartFailure::Rejected { error, .. }
        | hyper_os::task::StartFailure::Committed(error) => error,
    })?;
    let deadline = hyper_os::time::deadline_after(HANDOFF_TIMEOUT)?.as_raw();
    let result = commit_when_ready(
        runtime,
        manifest,
        &candidate_group,
        &process,
        &sender,
        &offer,
        deadline,
    );
    match result {
        Ok(true) => {
            // Process owners have moved. No further fallible operation here can
            // turn success into a rollback; an ACK is deliberately unnecessary.
            let _ = runtime
                .launcher
                .authorities
                .console
                .as_emergency_console()
                .write_all(
                    b"HypeR bootstrap-stage1-init: core supervision transferred; awaiting system retirement\n",
                );
            Ok(true)
        }
        other => {
            // The staging group contains only init. Never stop the core
            // group on failed preparation, timeout, or peer loss.
            let _ = process.as_process_supervisor().request_stop();
            let join_deadline = hyper_os::time::deadline_after(Duration::from_secs(5))?.as_raw();
            process
                .as_process_supervisor()
                .wait_terminated(join_deadline)?;
            other
        }
    }
}

fn commit_when_ready(
    runtime: &mut Runtime,
    manifest: &Manifest<'_>,
    group: &OwnedHandle<TaskGroupObject>,
    candidate: &OwnedHandle<ProcessObject>,
    channel: &CapabilityChannel,
    offer: &PreparedOffer<'_>,
    deadline: u64,
) -> hyper_os::Result<bool> {
    // Retain terminal optional Process owners until commit/abort: removing one
    // would change the already advertised receive-slot contract.
    let mut observed = [false; MAX_SERVICES];
    loop {
        let mut waits = reserved_vec(MAX_SERVICES + 2)?;
        let mut indexes = [0; MAX_SERVICES];
        let mut process_count = 0;
        for (index, process) in runtime.supervisors.processes.iter().enumerate() {
            if let Some(process) = process
                && !observed[index]
            {
                indexes[process_count] = index;
                process_count += 1;
                waits.push(WaitItem::new(
                    process.as_handle_ref(),
                    ObjectSignals::<ProcessObject>::TERMINATED,
                ));
            }
        }
        let candidate_index = waits.len();
        waits.push(WaitItem::new(
            candidate.as_handle_ref(),
            ObjectSignals::<ProcessObject>::TERMINATED,
        ));
        let channel_index = waits.len();
        waits.push(WaitItem::new(
            channel.as_handle_ref(),
            ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
                .union(ObjectSignals::<CapabilityChannelObject>::PEER_CLOSED),
        ));
        let observation = wait_many(&waits, deadline)?;
        if observation.index < candidate_index {
            let index = indexes[observation.index];
            observed[index] = true;
            if manifest
                .service(index)
                .is_none_or(|service| service.critical())
            {
                return Ok(false);
            }
            continue;
        }
        if observation.index == candidate_index {
            return Ok(false);
        }
        if observation.index != channel_index
            || !ObjectSignals::<CapabilityChannelObject>::PEER_RECEIVING
                .is_present_in(observation.observed)
        {
            return Ok(false);
        }
        drop(waits);
        let mut inventory = InventoryOffer::new(&runtime.launcher.authorities, group)?;
        inventory.describe_transition(
            runtime
                .root_transition
                .as_ref()
                .ok_or(hyper_os::Error::MissingHandle)?,
        )?;
        inventory.describe_processes(&runtime.supervisors)?;
        if inventory.slots != offer.slots {
            return Err(hyper_os::Error::InvalidResponse);
        }
        inventory
            .dispositions
            .push(CapabilityDisposition::move_handle(
                &mut runtime.root_transition,
                RightsOffer::SameRights,
            )?);
        // Borrow supervisors separately from the immutable management inventory.
        for process in runtime
            .supervisors
            .processes
            .iter_mut()
            .filter(|p| p.is_some())
        {
            inventory
                .dispositions
                .push(CapabilityDisposition::move_handle(
                    process,
                    RightsOffer::SameRights,
                )?);
        }
        match channel.try_send(COMMIT, &mut inventory.dispositions) {
            Ok(()) => return Ok(true),
            Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Bounded transaction buffers fail before capability ownership can move.
fn reserved_vec<T>(capacity: usize) -> hyper_os::Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| hyper_os::Error::Status(hyper_os::Status::NO_MEMORY))?;
    Ok(values)
}
