// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

mod broker;
mod physical_network;
mod storage;

#[cfg(feature = "physical-retirement-probe")]
#[path = "../../tests/fixtures/physical_retirement.rs"]
mod physical_retirement;

use hyper_io_runtime::guest_log::GuestLog;
use hyper_os::guest_io::Mailbox;
use hyper_os::handle::{ByteChannelObject, GuestMailboxObject, OwnedHandle, VirtualMachineObject};
use hyper_os::startup::{self, Startup};
use hyper_os::wait::{self, ObjectSignals, WaitItem};
use hyper_os::{device, vm};
use hyper_vm_image::guest_fdt::{
    GuestHardwareMetadata,
    io::{DmaRange, IoDevices, MmioDevice, MmioWindow, SdhciDevice, SdhciRevision},
};
use hyper_vm_support::io_guest::PhysicalAssignment;
use hyper_vm_support::io_guest::{self, Image, InstalledGuest, PHYSICAL_MMIO, RAM_BASE, RAM_BYTES};
use hyper_vm_support::io_protocol::{Command, MAX_RECORD, Reply, Request, Status};
use physical_network::PhysicalNetwork;
use std::io;
use std::process::ExitCode;
use std::time::Duration;

type Result<T> = std::result::Result<T, String>;
const MAILBOX_MMIO: u64 = 0x0a01_0000;
fn show(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}
fn deadline(seconds: u64) -> Result<u64> {
    hyper_os::time::deadline_after(Duration::from_secs(seconds))
        .map(|deadline| deadline.as_raw())
        .map_err(show)
}

/// Check operation time independently of wait readiness. Call only after
/// observing its completion, so an already queued reply still wins.
fn check_deadline(limit: u64) -> Result<()> {
    let now = hyper_os::time::monotonic_now()
        .map_err(show)?
        .as_nanoseconds();
    if hyper_io_runtime::deadline::expired(now, limit) {
        Err("I/O operation timed out".into())
    } else {
        Ok(())
    }
}

/// Loads the board-configured I/O VM and owns its physical-device lifetime.
///
/// After installation, every failure goes through guest stop/retirement,
/// hardware-worker shutdown and final log draining. Cleanup attempts preserve
/// the original failure rather than replacing it with a later cleanup error.
fn run(
    startup: &mut Startup<'_>,
    readiness: &mut Option<OwnedHandle<ByteChannelObject>>,
) -> Result<()> {
    let environment = hyper_os::fs::Directory::from_handle(
        startup
            .take(hyper_service::process::ENVIRONMENT_DIRECTORY)
            .map_err(show)?,
    );
    let board = environment
        .open("etc/hyper/board.json", hyper_os::fs::FileRights::READ)
        .map_err(show)?;
    let config = hyper_io_runtime::config::Config::read(board.into_std())?;
    let storage_policy = hyper_io_runtime::storage_policy::load(&environment)?;
    let mut broker = startup
        .take_optional(hyper_service::io::BROKER_SERVER)
        .map_err(show)?
        .map(|endpoint| broker::Broker::load(endpoint, &environment, &storage_policy))
        .transpose()?;
    if broker.is_some() && readiness.is_none() {
        return Err("I/O broker requires managed storage readiness".into());
    }
    let mut filesystem = startup
        .take_optional(hyper_service::filesystem::PROVIDER)
        .map_err(show)?;
    if filesystem.is_some() != readiness.is_some() {
        return Err("managed storage requires a filesystem provider".into());
    }
    let authority = startup
        .borrow(startup::DEVICE_ASSIGNMENT_AUTHORITY)
        .map_err(show)?;
    let policy = config.device();
    let (physical, sdhci_profile) = match policy.profile() {
        device::Profile::Userspace => {
            let (physical, profile) =
                hyper_io_runtime::sdhci::claim(authority, policy.identity()).map_err(show)?;
            (physical, Some(profile))
        }
        device::Profile::VirtioMmioScsi => {
            #[cfg(feature = "userspace-device-test")]
            let physical = hyper_io_runtime::sdhci::claim_virtio_test(authority, policy.identity())
                .map_err(show)?;
            #[cfg(not(feature = "userspace-device-test"))]
            let physical = device::claim_matching(authority, policy.profile(), policy.identity())
                .map_err(show)?;
            (physical, None)
        }
        device::Profile::VirtioMmioNet | device::Profile::PciFunction => {
            return Err("network controller selected for storage".into());
        }
    };
    let profile = device::profile_info(physical.as_handle_ref()).map_err(show)?;
    let network = PhysicalNetwork::claim(authority, config.network_device())?;
    let mut clients = Vec::new();
    if filesystem.is_some() {
        if storage_policy.native.is_empty() {
            return Err("filesystem provider requires explicit Native storage policy".into());
        }
        let root = startup.borrow(startup::ROOT_VMAR).map_err(show)?;
        clients.push(storage::NativeStorage::prepare(authority, root, 0, None)?);
        for policy in &storage_policy.native {
            clients.push(storage::NativeStorage::prepare(
                authority,
                root,
                policy.client,
                Some(policy.clone()),
            )?);
        }
    }
    let image_file = environment
        .open(
            config.definition().image.trim_start_matches('/'),
            hyper_os::fs::FileRights::READ.union(hyper_os::fs::FileRights::INSPECT),
        )
        .map_err(show)?;
    let image = Image::load(config.definition(), image_file.into_std())?;
    if image.plan.memory_size() != RAM_BYTES {
        return Err("resident I/O VM requires a 128 MiB configuration".into());
    }
    let dma =
        device::dma_extent(authority, image.memory.as_handle_ref(), 0, RAM_BYTES).map_err(show)?;
    let lease = vm::derive_creation_lease(
        startup
            .borrow(startup::VIRTUAL_MACHINE_CREATION_AUTHORITY)
            .map_err(show)?,
        startup.borrow(startup::RESOURCE_DOMAIN).map_err(show)?,
    )
    .map_err(show)?;
    let platform = vm::platform_info(lease.as_handle_ref(), vm::PlatformProfile::Aarch64Reference)
        .map_err(show)?;
    let metadata = hyper_vm_support::profile::validate_metadata(
        image.plan.architecture(),
        image.plan.platform_profile(),
        platform,
    )
    .map_err(show)?;
    let GuestHardwareMetadata::Aarch64 { gic_version } = metadata else {
        return Err("expected AArch64 platform metadata".into());
    };
    drop(lease);
    let own_dma = DmaRange {
        dma_base: dma.physical_base,
        cpu_base: RAM_BASE,
        size: RAM_BYTES,
    };
    let resource = |index| -> Result<MmioWindow> {
        let window = device::resource_info(physical.as_handle_ref(), index).map_err(show)?;
        if window.kind != index + 1 {
            return Err("unexpected physical resource kind".into());
        }
        Ok(MmioWindow {
            base: PHYSICAL_MMIO + window.offset,
            size: window.length,
        })
    };
    let host = resource(0)?;
    let host = MmioDevice {
        base: host.base,
        size: host.size,
        irq: 40,
    };
    let (storage_node, sdhci) = match (profile.profile, sdhci_profile) {
        (device::Profile::VirtioMmioScsi, None) if profile.resource_count == 1 => {
            (Some(host), None)
        }
        #[cfg(feature = "userspace-device-test")]
        (device::Profile::Userspace, None) if profile.resource_count == 1 => (Some(host), None),
        (device::Profile::Userspace, Some(metadata)) if profile.resource_count == 5 => (
            None,
            Some(SdhciDevice {
                host,
                config: resource(1)?,
                main_pinctrl: resource(2)?,
                aon_pinctrl: resource(3)?,
                aon_gpio: resource(4)?,
                revision: match metadata.revision {
                    0 => SdhciRevision::C0,
                    1 => SdhciRevision::D0,
                    _ => return Err("unsupported SDHCI revision".into()),
                },
                clock_hz: metadata.clock_hz,
                gpio_widths: metadata.gpio_widths,
            }),
        ),
        _ => return Err("invalid physical resource bundle".into()),
    };
    network.with_description(|network_node, pci| -> Result<()> {
        if !clients.is_empty() {
            let mut descriptions: Vec<_> =
                clients.iter().map(|client| client.description()).collect();
            let mut dma_ranges = vec![own_dma];
            dma_ranges.extend(clients.iter().map(|client| client.dma_range()));
            if let Some(broker) = broker.as_ref() {
                descriptions.extend(broker.descriptions());
                dma_ranges.extend(broker.dma_ranges(&dma_ranges)?);
            }
            image.device_tree(
                gic_version,
                IoDevices {
                    clients: &descriptions,
                    virtio: storage_node,
                    network: network_node,
                    pci,
                    sdhci,
                    dma_ranges: &dma_ranges,
                    ..IoDevices::empty()
                },
            )
        } else {
            image.device_tree(
                gic_version,
                IoDevices {
                    clients: &[],
                    virtio: storage_node,
                    network: network_node,
                    pci,
                    sdhci,
                    dma_ranges: &[own_dma],
                    mailbox: Some(MmioDevice {
                        base: MAILBOX_MMIO,
                        size: 4096,
                        irq: 41,
                    }),
                    ..IoDevices::empty()
                },
            )
        }
    })?;
    let grant = vm::create_guest_memory(image.memory.as_handle_ref()).map_err(show)?;
    let mappings: Vec<_> = clients.iter().map(|client| client.mapping()).collect();
    let mut assignments = vec![PhysicalAssignment {
        device: physical.as_handle_ref(),
        base: PHYSICAL_MMIO,
        irq: 40,
    }];
    if let Some(assignment) = network.assignment() {
        assignments.push(assignment);
    }
    let mut guest = io_guest::install_mapped(
        startup,
        &image,
        &grant,
        mappings.as_slice(),
        if broker.is_some() {
            vm::DYNAMIC_ALIAS_OFFSET + vm::DYNAMIC_PHYSICAL_LIMIT - RAM_BASE
        } else {
            clients
                .iter()
                .map(|client| client.address_space_end())
                .max()
                .unwrap_or(RAM_BYTES)
        },
        &assignments,
        0xd000_0000,
    )?;
    drop(grant);
    // Every fallible operation after installation is inside this result scope;
    // retirement runs even if mailbox creation, start or negotiation fails.
    let mut hardware_worker = None;
    let mut guest_log = GuestLog::default();
    let outcome = (|| {
        let mut mailboxes = Vec::new();
        if clients.is_empty() {
            mailboxes.push(
                Mailbox::create(guest.machine.as_handle_ref(), MAILBOX_MMIO, 41).map_err(show)?,
            );
        }
        for client in &mut clients {
            let description = client.description();
            mailboxes.push(
                Mailbox::create(
                    guest.machine.as_handle_ref(),
                    description.mailbox.base,
                    description.mailbox.irq,
                )
                .map_err(show)?,
            );
            client.install(&guest)?;
        }
        if let Some(broker) = broker.as_mut() {
            broker.install(&guest)?;
        }
        if profile.profile == device::Profile::Userspace {
            use hyper_io_runtime::sdhci::worker;
            vm::register_mmio(
                guest.machine.as_handle_ref(),
                PHYSICAL_MMIO,
                65536,
                core::num::NonZeroU64::new(worker::DEVICE_COOKIE).ok_or("invalid device cookie")?,
            )
            .map_err(|error| format!("register physical MMIO: {error:?}"))?;
            hardware_worker = Some(if sdhci.is_some() {
                worker::start(&physical, &guest)
                    .map_err(|error| format!("start SDHCI worker: {error:?}"))?
            } else {
                #[cfg(feature = "userspace-device-test")]
                {
                    worker::start_virtio_test(&physical, &guest)
                        .map_err(|error| format!("start test device worker: {error:?}"))?
                }
                #[cfg(not(feature = "userspace-device-test"))]
                {
                    return Err("missing userspace physical device driver".into());
                }
            });
        }
        #[cfg(feature = "userspace-device-test")]
        println!("DEVICE-TEST: worker prepared");
        vm::start_vcpu(guest.cpus[0].as_handle_ref()).map_err(show)?;
        supervise(
            &mut guest,
            &mut guest_log,
            StorageBootstrap {
                clients: &mut clients,
                mailboxes: &mailboxes,
                policy: &storage_policy,
                provider: filesystem
                    .take()
                    .map(hyper_os::capability_channel::CapabilityChannel::from_handle),
            },
            broker.as_mut(),
            readiness,
        )
    })();
    if let Err(error) = &outcome {
        eprintln!("HypeR io-runtime: retiring after failure: {error}");
    }
    let stopped = vm::request_stop(guest.machine.as_handle_ref())
        .map_err(show)
        .and_then(|()| {
            vm::wait_terminated(guest.machine.as_handle_ref(), deadline(20)?).map_err(show)
        });
    let worker_stopped = hardware_worker
        .map(|worker| worker.stop().map_err(show))
        .transpose()
        .map(|_| ());
    let drained = drain(&mut guest, &mut guest_log);
    // Attempt every cleanup step without hiding the failure that started
    // retirement behind a later timeout or worker shutdown error.
    outcome.and(stopped).and(worker_stopped).and(drained)
}

fn drain(guest: &mut InstalledGuest, guest_log: &mut GuestLog) -> Result<()> {
    let mut bytes = [0; 2048];
    let mut output = io::stdout().lock();
    for _ in 0..16 {
        let length = guest.output.try_read(&mut bytes).map_err(show)?;
        if length == 0 {
            break;
        }
        guest_log
            .write(&mut output, &bytes[..length])
            .map_err(show)?;
    }
    Ok(())
}

struct StorageBootstrap<'a> {
    clients: &'a mut [storage::NativeStorage],
    mailboxes: &'a [Mailbox],
    policy: &'a hyper_io_runtime::storage_policy::Policy,
    provider: Option<hyper_os::capability_channel::CapabilityChannel>,
}

/// Negotiates the backend, publishes storage readiness, and services its clients.
///
/// With Native storage configured, consume the readiness endpoint only after
/// mounting the configured volume and sending the ready record. Until then the outer owner
/// retains it through failure cleanup. Broker progress shares this loop with
/// guest console and power events instead of blocking on individual clients.
fn supervise(
    guest: &mut InstalledGuest,
    guest_log: &mut GuestLog,
    storage: StorageBootstrap<'_>,
    mut broker: Option<&mut broker::Broker>,
    readiness: &mut Option<OwnedHandle<ByteChannelObject>>,
) -> Result<()> {
    let StorageBootstrap {
        clients,
        mailboxes,
        policy: storage_policy,
        provider: mut filesystem,
    } = storage;
    // HELLO has no DMA/queue side effects. There is deliberately no periodic
    // health request once the backend acknowledges its startup contract.
    let mailbox = mailboxes.first().ok_or("missing backend control mailbox")?;
    let hello = Request {
        binding: 1,
        epoch: 1,
        transaction: 1,
        command: Command::Hello,
    };
    let mut record = [0; MAX_RECORD];
    let management = filesystem
        .as_ref()
        .map(|_| hyper_vm_support::block_protocol::Request {
            transaction: 1,
            command: hyper_vm_support::block_protocol::Command::Hello,
        });
    let length = match management {
        Some(request) => request.encode(&mut record).map_err(show)?,
        None => hello.encode(&mut record).map_err(show)?,
    };
    mailbox.send(&record[..length]).map_err(show)?;
    let startup_deadline = deadline(60)?;
    let mut ready = false;
    loop {
        if !pump_guest(guest, guest_log)? {
            return Ok(());
        }
        if !ready {
            match mailbox.receive(&mut record) {
                Ok(length) => {
                    let max_read = if let Some(request) = management {
                        let hyper_vm_support::block_protocol::Reply::Hello { max_read } =
                            request.decode_reply(&record[..length]).map_err(show)?
                        else {
                            return Err("management HELLO omitted block capabilities".into());
                        };
                        max_read
                    } else {
                        let reply = Reply::decode(&record[..length], hello).map_err(show)?;
                        if reply.status != Status::Success {
                            return Err("backend refused HELLO".into());
                        }
                        0
                    };
                    ready = true;
                    if let Some(provider) = filesystem.take() {
                        let mounted = storage::mount_volumes(
                            clients,
                            mailboxes,
                            provider,
                            guest,
                            guest_log,
                            storage_policy,
                            max_read,
                        )?;
                        let writer = readiness
                            .as_ref()
                            .ok_or("readiness capability already consumed")?;
                        if let Err(error) = writer.as_byte_channel().send(if mounted {
                            hyper_service::io::READY_MESSAGE
                        } else {
                            hyper_service::filesystem::FAILURE_MESSAGE
                        }) {
                            eprintln!(
                                "HypeR io-runtime: readiness receiver unavailable: {error:?}"
                            );
                        }
                        drop(readiness.take());
                        if mounted {
                            println!("HypeR io-runtime: ready; Native volumes mounted");
                        } else {
                            eprintln!(
                                "HypeR io-runtime: required mount unavailable; backend remains active"
                            );
                        }
                    } else {
                        println!(
                            "HypeR io-runtime: ready; storage backend idle (no client attached)"
                        );
                    }
                    #[cfg(feature = "physical-retirement-probe")]
                    physical_retirement::arm()?;
                }
                Err(hyper_os::Error::Status(hyper_os::Status::WOULD_BLOCK)) => {}
                Err(error) => return Err(show(error)),
            }
        }
        if !ready {
            check_deadline(startup_deadline)?;
        }
        if ready
            && let Some(broker) = broker.as_mut()
            && broker.service(guest)?
        {
            continue;
        }
        let mut items = vec![
            guest.output.wait_item(),
            WaitItem::new(
                guest.machine.as_handle_ref(),
                ObjectSignals::<VirtualMachineObject>::POWER_REQUEST
                    .union(ObjectSignals::<VirtualMachineObject>::VCPU_TERMINATED),
            ),
            WaitItem::new(
                mailbox.as_handle_ref(),
                ObjectSignals::<GuestMailboxObject>::READABLE
                    .union(ObjectSignals::<GuestMailboxObject>::PEER_CLOSED),
            ),
        ];
        if ready && let Some(broker) = broker.as_ref() {
            items.extend(broker.wait_items());
        }
        let observed = match wait::wait_many(
            &items,
            if ready {
                broker
                    .as_ref()
                    .map_or(hyper_os::DEADLINE_INFINITE, |broker| broker.next_deadline())
            } else {
                startup_deadline
            },
        ) {
            Ok(observed) => observed,
            Err(hyper_os::Error::Status(hyper_os::Status::TIMED_OUT)) if ready => continue,
            Err(error) => return Err(show(error)),
        };
        if observed.index == 2 {
            if ObjectSignals::<GuestMailboxObject>::PEER_CLOSED.is_present_in(observed.observed) {
                return Err("backend control channel closed".into());
            }
            if ready {
                return Err("unexpected message from idle backend".into());
            }
        }
    }
}

fn pump_guest(guest: &mut InstalledGuest, guest_log: &mut GuestLog) -> Result<bool> {
    drain(guest, guest_log)?;
    for cpu in &guest.cpus {
        if let Some(reason) = vm::vcpu_info(cpu.as_handle_ref()).map_err(show)?.terminal {
            return Err(format!("I/O vCPU terminated: {reason:?}"));
        }
    }
    for _ in 0..guest.cpus.len() {
        let Some(request) =
            vm::pending_power_request(guest.machine.as_handle_ref()).map_err(show)?
        else {
            break;
        };
        match request.operation {
            vm::PowerOperation::CpuOn | vm::PowerOperation::CpuOff => {
                vm::complete_power_request(guest.machine.as_handle_ref(), request.id, true)
                    .map_err(show)?
            }
            vm::PowerOperation::SystemOff | vm::PowerOperation::SystemReset => {
                eprintln!("HypeR io-runtime: backend requested shutdown");
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(super) fn main() -> ExitCode {
    let mut startup = match hyper_rt::process::startup() {
        Ok(startup) => startup,
        Err(error) => {
            eprintln!("HypeR io-runtime: startup failed: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    let mut readiness = match startup.take_optional(hyper_service::io::READY) {
        Ok(readiness) => readiness,
        Err(error) => {
            eprintln!("HypeR io-runtime: failed: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    let status = match run(&mut startup, &mut readiness) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("HypeR io-runtime: failed: {error}");
            ExitCode::FAILURE
        }
    };
    // Closing READY tells init to abandon this provider. Queue the complete
    // failure diagnostic first so its targeted stop cannot truncate the cause.
    drop(readiness);
    status
}
