// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap stops at transfer; only the successor controls retirement.
use crate::contracts::{filesystem, io::send_capabilities};
use crate::{manager::Manager, show};
use core::mem::MaybeUninit;
use hyper_fs_backend::transfer::{self, Description, Mount, Volume};
use hyper_os::{
    capability_channel::{CapabilityChannel, CapabilityDisposition, CapabilityReceiveSlot},
    handle::{
        BlockQueueObject, ByteChannelObject, NativeBlockObject, Rights, RightsOffer, VmoObject,
    },
    memory::{PAGE_SIZE, SnapshotVmo, WritableVmo},
};

pub(super) fn export(
    mut manager: Manager<'_>,
    control: CapabilityChannel,
    endpoint: CapabilityChannel,
) -> Result<(), String> {
    if let Err(error) = prepare(&manager, &endpoint) {
        eprintln!(
            "{}adoption preparation failed; bootstrap mounts retained: {error}",
            crate::logging::prefix("fs-backend")
        );
        // No live authority has moved. Peer loss or malformed preparation may
        // not retire healthy workers or close the mounted filesystem channels.
        drop(endpoint);
        return crate::serve_control(manager, control);
    }
    // All replacements and their DSOs are resident before the first original
    // worker yields. Therefore loading a replacement cannot re-enter /system.
    let mut retired_worker_leases = Vec::new();
    for volume in &mut manager.volumes {
        if let Some(worker) = volume.worker.take() {
            worker
                .owner
                .as_byte_channel()
                .send(filesystem::WORKER_HANDOFF)
                .map_err(show)?;
            let mut slots = [
                CapabilityReceiveSlot::new::<NativeBlockObject>(hyper_os::block::RIGHTS),
                CapabilityReceiveSlot::new::<ByteChannelObject>(filesystem::TRANSPORT_RIGHTS),
                CapabilityReceiveSlot::new::<VmoObject>(filesystem::SERVER_BUFFER_RIGHTS),
                CapabilityReceiveSlot::new::<BlockQueueObject>(filesystem::QUEUE_RIGHTS),
            ];
            let mut bytes = [MaybeUninit::uninit(); 16];
            let message = worker
                .control
                .receive(crate::deadline()?, &mut bytes, &mut slots)
                .map_err(show)?;
            if message.bytes().len() != 16
                || &message.bytes()[..8] != filesystem::WORKER_STATE
                || message.capability_count() != 4
            {
                return Err("invalid drained filesystem worker state".into());
            }
            let mut block = slots[0].take::<NativeBlockObject>().map_err(show)?;
            let mut server = slots[1].take::<ByteChannelObject>().map_err(show)?;
            let mut shared = slots[2].take::<VmoObject>().map_err(show)?;
            let mut queue = slots[3].take::<BlockQueueObject>().map_err(show)?;
            retired_worker_leases.push(worker.export(
                &endpoint,
                message.bytes(),
                &mut block,
                &mut server,
                &mut shared,
                &mut queue,
            )?);
        } else {
            let mut block = Some(
                volume
                    .block
                    .take()
                    .ok_or("missing exclusive volume")?
                    .into_handle(),
            );
            let mut offers = [CapabilityDisposition::move_handle(
                &mut block,
                RightsOffer::Exact(hyper_os::block::RIGHTS),
            )
            .map_err(show)?];
            send_capabilities(
                &endpoint,
                transfer::VOLUME,
                &mut offers,
                hyper_os::DEADLINE_INFINITE,
            )
            .map_err(show)?;
        }
    }
    let mut control = Some(control.into_handle());
    let mut offers = [CapabilityDisposition::move_handle(
        &mut control,
        RightsOffer::Exact(Rights::READ.union(Rights::WAIT).union(Rights::TRANSFER)),
    )
    .map_err(show)?];
    send_capabilities(
        &endpoint,
        transfer::FINISH,
        &mut offers,
        hyper_os::DEADLINE_INFINITE,
    )
    .map_err(show)?;
    drop(endpoint);
    // Keep the predecessor groups active until system init stops their Process
    // owners. Neither adoption nor the success reply retires a boot service.
    let _leases = retired_worker_leases;
    let _environment = manager;
    loop {
        std::thread::park();
    }
}

fn prepare(manager: &Manager<'_>, endpoint: &CapabilityChannel) -> Result<(), String> {
    let description = Description {
        version: 1,
        volumes: manager
            .volumes
            .iter()
            .map(|volume| Volume {
                id: volume.id.0,
                label: volume.label.clone(),
                sectors: volume.geometry.sectors,
                read_only: volume.geometry.read_only,
                mounted: volume.worker.as_ref().map(|worker| Mount {
                    mount_id: worker.mount_id.unwrap_or(0),
                    target: worker.target.clone(),
                    format: worker.format.name().to_ascii_lowercase(),
                    read_only: worker.read_only,
                }),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec(&description).map_err(show)?;
    Description::parse(&bytes)?;
    let memory = WritableVmo::create((bytes.len() as u64 + 8).div_ceil(PAGE_SIZE) * PAGE_SIZE)
        .map_err(show)?;
    memory
        .write_all_at(0, &(bytes.len() as u64).to_le_bytes())
        .map_err(show)?;
    memory.write_all_at(8, &bytes).map_err(show)?;
    let snapshot = SnapshotVmo::from_vmo(memory.as_handle_ref()).map_err(show)?;
    let mut offers = [CapabilityDisposition::duplicate(
        snapshot.as_handle_ref(),
        RightsOffer::Exact(Rights::READ.union(Rights::INSPECT).union(Rights::TRANSFER)),
    )
    .map_err(show)?];
    send_capabilities(
        endpoint,
        transfer::DESCRIBE,
        &mut offers,
        hyper_os::DEADLINE_INFINITE,
    )
    .map_err(show)?;
    drop(snapshot);
    drop(memory);
    let mut bytes = [MaybeUninit::uninit(); 64];
    let message = endpoint
        .receive(hyper_os::DEADLINE_INFINITE, &mut bytes, &mut [])
        .map_err(show)?;
    if message.bytes() != transfer::PREPARED || message.capability_count() != 0 {
        return Err("invalid filesystem adoption preparation".into());
    }
    Ok(())
}
