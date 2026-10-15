// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Worker ownership transactions. Publication belongs to the manager, and a
//! clean return transfers the original `NativeBlock` only after namespace detach.

use crate::contracts::filesystem;
use crate::format::Format;
use crate::{deadline, filesystem_error, show};
use core::mem::MaybeUninit;
use hyper_fs_backend::fstab::Entry;
use hyper_fs_backend::worker::Program;
use hyper_os::{
    block::{NativeBlock, VolumeInfo},
    capability_channel::{CapabilityChannel, CapabilityReceiveSlot},
    fs::{Directory, File},
    handle::{
        ByteChannelObject, NativeBlockObject, OwnedHandle, ProcessObject, ResourceDomainObject,
        Rights, RightsOffer, TaskGroupObject, VmoObject,
    },
    startup::{self, Startup},
    task::{
        ProcessBuilder, ProcessPhase, ResourceLimits, create_resource_domain, create_task_group,
    },
};

const LIMITS: ResourceLimits = ResourceLimits {
    kernel_memory_bytes: 64 * 1024 * 1024,
    processes: 1,
    threads: 4,
    handles: 64,
    kernel_objects: 256,
    committed_pages: 32 * 1024,
    pinned_pages: 1024,
    guest_pages: 0,
    ipc_messages: 32,
    ipc_bytes: 1024 * 1024,
    ipc_handles: 16,
    subscriptions: 32,
    timers: 32,
    virtual_machines: 0,
    virtual_cpus: 0,
    device_leases: 0,
    dma_mappings: 0,
    user_address_spaces: 1,
    user_mappings: 256,
};

pub(super) struct Worker {
    pub(super) child: OwnedHandle<ProcessObject>,
    _group: OwnedHandle<TaskGroupObject>,
    _domain: OwnedHandle<ResourceDomainObject>,
    pub(super) control: CapabilityChannel,
    pub(super) owner: OwnedHandle<ByteChannelObject>,
    pub(super) channel: Option<OwnedHandle<ByteChannelObject>>,
    pub(super) buffer: Option<OwnedHandle<VmoObject>>,
    pub(super) published: bool,
    pub(super) mount_id: Option<u64>,
    pub target: String,
    pub format: Format,
    pub read_only: bool,
}
impl Worker {
    pub fn start(
        startup: &Startup<'_>,
        _root: &Directory,
        program: Program<File>,
        format: Format,
        geometry: VolumeInfo,
        entry: &Entry,
        block: &mut Option<NativeBlock>,
    ) -> Result<Self, String> {
        let Program {
            path,
            source: _,
            image,
        } = program;
        let factory = startup.borrow(startup::TASK_FACTORY).map_err(show)?;
        let domain = create_resource_domain(
            startup.borrow(startup::RESOURCE_DOMAIN).map_err(show)?,
            LIMITS,
        )
        .map_err(show)?;
        let group = create_task_group(factory, domain.as_handle_ref()).map_err(show)?;
        // Bootstrap workers are always static, even if policy names a file on
        // another mounted volume. Their executable location does not select a
        // different userspace loader or a formal library environment.
        let loader = startup
            .borrow(crate::contracts::process::CHILD_LOADER)
            .map_err(show)?;
        let builder = ProcessBuilder::create(
            factory,
            group.as_handle_ref(),
            domain.as_handle_ref(),
            image.as_handle_ref(),
            loader,
        )
        .map_err(show)?;
        builder.set_name(format.worker_name()).map_err(show)?;
        for argument in [
            path.as_str(),
            &geometry.sectors.to_string(),
            if entry.read_only { "ro" } else { "rw" },
        ] {
            builder.add_argument(argument).map_err(show)?;
        }
        let libraries = startup
            .borrow(crate::contracts::process::CHILD_LIBRARY_DIRECTORY)
            .map_err(show)?;
        builder
            .add_handle_duplicate(
                libraries,
                startup::DYNAMIC_LIBRARY_DIRECTORY.as_raw(),
                RightsOffer::Exact(Rights::READ.union(Rights::EXECUTE).union(Rights::PROTECT)),
            )
            .map_err(show)?;
        builder
            .add_handle_duplicate(
                domain.as_handle_ref(),
                startup::RESOURCE_DOMAIN.as_raw(),
                RightsOffer::Exact(Rights::RESOURCE_DOMAIN_SPONSOR),
            )
            .map_err(show)?;
        for (output, contract) in [
            (
                hyper_rt::process::stdout().map_err(show)?,
                crate::contracts::stdio::STANDARD_OUTPUT_CONTRACT,
            ),
            (
                hyper_rt::process::stderr().map_err(show)?,
                crate::contracts::stdio::STANDARD_ERROR_CONTRACT,
            ),
        ] {
            builder
                .add_handle_duplicate(
                    output.as_handle_ref(),
                    contract.purpose(),
                    RightsOffer::Exact(contract.required_rights()),
                )
                .map_err(show)?;
        }
        let (control, child_control) = CapabilityChannel::create().map_err(show)?;
        let (owner, child_owner) = hyper_os::channel::create_pair().map_err(show)?;
        builder
            .add_handle_move(
                child_control.into_handle(),
                filesystem::WORKER_CONTROL.as_raw(),
                RightsOffer::Exact(Rights::READ.union(Rights::WRITE).union(Rights::WAIT)),
            )
            .map_err(|error| show(error.error()))?;
        builder
            .add_handle_move(
                child_owner,
                filesystem::OWNER.as_raw(),
                RightsOffer::Exact(Rights::READ.union(Rights::WAIT)),
            )
            .map_err(|error| show(error.error()))?;
        // The exclusive block is the final setup transfer; failures before here
        // retain it in the manager and are retryable without reactivation.
        let device = block.take().ok_or("volume already delegated")?;
        if let Err(error) = builder.add_handle_move(
            device.into_handle(),
            filesystem::BLOCK.as_raw(),
            RightsOffer::Exact(hyper_os::block::RIGHTS),
        ) {
            let (error, handle) = error.into_parts();
            *block = Some(NativeBlock::from_handle(handle));
            return Err(show(error));
        }
        builder.seal().map_err(show)?;
        let child = builder
            .start()
            .map_err(|failure| format!("worker start failed: {:?}", failure.error()))?;
        let prepared = (|| {
            let mut bytes = [MaybeUninit::uninit(); 16];
            let mut slots = [
                CapabilityReceiveSlot::new::<ByteChannelObject>(filesystem::TRANSPORT_RIGHTS),
                CapabilityReceiveSlot::new::<VmoObject>(filesystem::BUFFER_RIGHTS),
            ];
            let message = control
                .receive(deadline()?, &mut bytes, &mut slots)
                .map_err(show)?;
            if message.bytes() != filesystem::WORKER_READY || message.capability_count() != 2 {
                return Err("invalid filesystem worker handshake".into());
            }
            let channel = slots[0]
                .take::<ByteChannelObject>()
                .map_err(show)?
                .ok_or("missing filesystem endpoint")?;
            let buffer = slots[1]
                .take::<VmoObject>()
                .map_err(show)?
                .ok_or("missing filesystem buffer")?;
            Ok((channel, buffer))
        })();
        let (channel, buffer) = match prepared {
            Ok(value) => value,
            Err(error) => {
                drop(owner);
                drop(control);
                let _ = child.as_process_supervisor().request_stop();
                let _ = child.as_process_supervisor().wait_terminated(deadline()?);
                return Err(error);
            }
        };
        Ok(Self {
            child,
            _group: group,
            _domain: domain,
            control,
            owner,
            channel: Some(channel),
            buffer: Some(buffer),
            published: false,
            mount_id: None,
            target: entry.target.clone(),
            format,
            read_only: entry.read_only,
        })
    }
    pub fn publish(&mut self, root: &Directory, read_only: bool) -> Result<(), String> {
        hyper_os::filesystem::mount(
            self.channel
                .as_ref()
                .ok_or("missing filesystem transport")?
                .as_handle_ref(),
            self.buffer
                .as_ref()
                .ok_or("missing filesystem buffer")?
                .as_handle_ref(),
            root.as_handle_ref(),
            &self.target,
            read_only,
        )
        .map_err(filesystem_error)?;
        self.published = true;
        self.mount_id = Some(
            root.open_directory(
                &self.target,
                hyper_os::fs::DirectoryRights::READ.union(hyper_os::fs::DirectoryRights::INSPECT),
            )
            .map_err(show)?
            .info()
            .map_err(show)?
            .location()
            .mount()
            .get(),
        );
        println!(
            "{}{} mounted at {}",
            crate::logging::prefix("fs-backend"),
            self.format.name(),
            self.target
        );
        Ok(())
    }
    pub(super) fn export(
        self,
        endpoint: &CapabilityChannel,
        record: &[u8],
        block: &mut Option<OwnedHandle<NativeBlockObject>>,
        server: &mut Option<OwnedHandle<ByteChannelObject>>,
        shared: &mut Option<OwnedHandle<VmoObject>>,
    ) -> Result<
        (
            OwnedHandle<TaskGroupObject>,
            OwnedHandle<ResourceDomainObject>,
        ),
        String,
    > {
        use hyper_os::capability_channel::CapabilityDisposition;
        let mut child = Some(self.child);
        let mut channel = self.channel;
        let mut buffer = self.buffer;
        let mut offers = [
            CapabilityDisposition::move_handle(block, RightsOffer::Exact(hyper_os::block::RIGHTS))
                .map_err(show)?,
            CapabilityDisposition::move_handle(
                server,
                RightsOffer::Exact(filesystem::TRANSPORT_RIGHTS),
            )
            .map_err(show)?,
            CapabilityDisposition::move_handle(
                shared,
                RightsOffer::Exact(filesystem::SERVER_BUFFER_RIGHTS),
            )
            .map_err(show)?,
            CapabilityDisposition::move_handle(
                &mut channel,
                RightsOffer::Exact(filesystem::TRANSPORT_RIGHTS),
            )
            .map_err(show)?,
            CapabilityDisposition::move_handle(
                &mut buffer,
                RightsOffer::Exact(filesystem::BUFFER_RIGHTS),
            )
            .map_err(show)?,
            CapabilityDisposition::move_handle(
                &mut child,
                RightsOffer::Exact(hyper_fs_backend::transfer::PROCESS_RIGHTS),
            )
            .map_err(show)?,
        ];
        crate::contracts::io::send_capabilities(
            endpoint,
            record,
            &mut offers,
            hyper_os::DEADLINE_INFINITE,
        )
        .map_err(show)?;
        Ok((self._group, self._domain))
    }

    pub fn finish(self) -> Result<NativeBlock, String> {
        self.owner
            .as_byte_channel()
            .send(filesystem::WORKER_STOP)
            .map_err(show)?;
        let mut bytes = [MaybeUninit::uninit(); 16];
        let mut slots = [CapabilityReceiveSlot::new::<NativeBlockObject>(
            hyper_os::block::RIGHTS,
        )];
        let message = self
            .control
            .receive(deadline()?, &mut bytes, &mut slots)
            .map_err(show)?;
        if message.bytes() != filesystem::WORKER_RETURN || message.capability_count() != 1 {
            return Err("worker did not return exclusive block ownership".into());
        }
        let block = slots[0]
            .take::<NativeBlockObject>()
            .map_err(show)?
            .ok_or("worker omitted block owner")?;
        let supervisor = self.child.as_process_supervisor();
        let retired_by = deadline()?;
        supervisor.wait_terminated(retired_by).map_err(show)?;
        // Termination stops execution before the reaper drops the worker's
        // handles and address space. Reuse only after that cleanup completes.
        while supervisor.info().map_err(show)?.phase != ProcessPhase::Retired {
            let now = hyper_os::time::monotonic_now()
                .map_err(show)?
                .as_nanoseconds();
            if now >= retired_by {
                return Err("filesystem worker retirement timed out".into());
            }
            hyper_os::thread::sleep_until(now.saturating_add(1_000_000).min(retired_by))
                .map_err(show)?;
        }
        Ok(NativeBlock::from_handle(block))
    }
}
