// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Named volume handoff; file requests never traverse the I/O runtime.

use crate::contracts::filesystem;
use hyper_io_runtime::storage_policy::Volume;
use hyper_os::block::{self, NativeBlock};
use hyper_os::capability_channel::{CapabilityChannel, CapabilityDisposition};
use hyper_os::handle::{ByteChannelObject, Rights, RightsOffer};
use hyper_os::wait::{self, ObjectSignals, WaitItem};

use super::super::{Result, check_deadline, deadline, show};

/// False means the devices remain usable but a required mount failed. The I/O
/// owner must keep them running so the recovery shell can repair the deployment.
pub(super) fn attach(
    provider: CapabilityChannel,
    volumes: Vec<(Volume, hyper_io_runtime::partition::Partition, NativeBlock)>,
) -> Result<bool> {
    let (ready, receiver) = hyper_os::channel::create_pair().map_err(show)?;
    let limit = deadline(crate::contracts::io::READY_TIMEOUT_SECONDS)?;
    let mut ready = Some(ready);
    crate::contracts::io::send_capabilities(
        &provider,
        filesystem::BEGIN_MESSAGE,
        &mut [CapabilityDisposition::move_handle(
            &mut ready,
            RightsOffer::Exact(Rights::WAIT.union(Rights::WRITE).union(Rights::TRANSFER)),
        )
        .map_err(show)?],
        limit,
    )
    .map_err(show)?;
    for (policy, partition, block) in volumes {
        let volume = block.activate(policy.read_only).map_err(show)?;
        if volume.sectors != partition.sectors {
            return Err("bound block capacity differs from discovered partition".into());
        }
        println!(
            "{}Native volume {}: {} sectors",
            crate::logging::prefix("io-runtime"),
            policy.source,
            volume.sectors
        );
        let request = filesystem::encode_attach(partition.id, &partition.label, volume)
            .ok_or("invalid named volume geometry")?;
        let mut block = Some(block.into_handle());
        crate::contracts::io::send_capabilities(
            &provider,
            &request,
            &mut [CapabilityDisposition::move_handle(
                &mut block,
                RightsOffer::Exact(block::RIGHTS),
            )
            .map_err(show)?],
            limit,
        )
        .map_err(show)?;
    }
    crate::contracts::io::send_capabilities(&provider, filesystem::FINISH_MESSAGE, &mut [], limit)
        .map_err(show)?;
    // All moves have committed. The manager owns the block capabilities, and
    // reports bootstrap fstab readiness over the one dedicated result pipe.
    let mut response = [0_u8; 32];
    loop {
        match receiver.as_byte_channel().try_receive(&mut response) {
            Ok(length) if &response[..length] == filesystem::READY_MESSAGE => return Ok(true),
            Ok(length) if &response[..length] == filesystem::FAILURE_MESSAGE => return Ok(false),
            Ok(_) => return Err("invalid filesystem readiness record".into()),
            Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => {}
            Err(error) => return Err(format!("filesystem manager failed: {error:?}")),
        }
        check_deadline(limit)?;
        let item = WaitItem::new(
            receiver.as_handle_ref(),
            ObjectSignals::<ByteChannelObject>::READABLE
                .union(ObjectSignals::<ByteChannelObject>::PEER_CLOSED),
        );
        wait::wait_many(&[item], limit).map_err(show)?;
    }
}
