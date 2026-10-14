// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Live bootstrap authority inventory and typed capability offers.

use crate::manifest::{CapabilityGrant, CapabilityOperation};
use hyper_os::fs::{Directory, File};
use hyper_os::handle::{
    ByteChannelObject, CapabilityChannelObject, ConsoleObject, CpuInspectorObject,
    DeviceAssignmentAuthorityObject, DirectoryObject, FileObject, MemoryInspectorObject,
    ObjectInspectorObject, OwnedHandle, ResourceDomainObject, Rights, RightsOffer,
    TaskFactoryObject, TaskGroupObject, TaskInspectorObject, TypedObject,
    VirtualMachineCreationAuthorityObject,
};
use hyper_os::task::ProcessBuilder;

use super::LaunchError;
use crate::bootstrap_policy::*;

/// Live bootstrap capabilities from which service offers are prepared.
pub(super) struct AuthorityInventory {
    pub(super) root_directory: Directory,
    pub(super) boot_environment: Directory,
    pub(super) system_environment: Option<Directory>,
    pub(super) library_directory: Directory,
    pub(super) factory: OwnedHandle<TaskFactoryObject>,
    pub(super) group: OwnedHandle<TaskGroupObject>,
    pub(super) domain: OwnedHandle<ResourceDomainObject>,
    pub(super) vm_creation: Option<OwnedHandle<VirtualMachineCreationAuthorityObject>>,
    pub(super) device: Option<OwnedHandle<DeviceAssignmentAuthorityObject>>,
    pub(super) task_inspector: OwnedHandle<TaskInspectorObject>,
    pub(super) object_inspector: OwnedHandle<ObjectInspectorObject>,
    pub(super) memory_inspector: OwnedHandle<MemoryInspectorObject>,
    pub(super) cpu_inspector: OwnedHandle<CpuInspectorObject>,
    pub(super) console: OwnedHandle<ConsoleObject>,
    pub(super) raw_control_server: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) raw_controller: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) log_server: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) log_connector: OwnedHandle<CapabilityChannelObject>,
    // Replaced for each launch; stdout/stderr of one service share one source.
    pub(super) service_output_channel: Option<OwnedHandle<ByteChannelObject>>,
    pub(super) console_drain: Option<OwnedHandle<ByteChannelObject>>,
    pub(super) io_handoff: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) filesystem_handoff: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) io_broker_server: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) io_broker_client: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) io_ready_channel: Option<OwnedHandle<ByteChannelObject>>,
    pub(super) filesystem_attach: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) filesystem_control_server: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) filesystem_control_client: Option<OwnedHandle<CapabilityChannelObject>>,
    pub(super) filesystem_provider: Option<OwnedHandle<CapabilityChannelObject>>,
}

impl AuthorityInventory {
    pub(super) fn offer(
        &mut self,
        grant: CapabilityGrant,
        builder: &ProcessBuilder,
        loader: &File,
        environment: &Directory,
    ) -> Result<(), LaunchError> {
        let authority =
            BootstrapAuthority::from_key(grant.authority()).ok_or(LaunchError::InvalidPlan)?;
        let operation = grant.operation();
        let kind = grant.object_kind();
        let purpose = grant.purpose();
        let rights = Rights::from_bits(grant.rights()).ok_or(LaunchError::InvalidPlan)?;
        let offer = RightsOffer::Exact(rights);
        match (authority, operation) {
            (BootstrapAuthority::ChildLoader, CapabilityOperation::Duplicate)
                if kind == FileObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(loader.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::Console, CapabilityOperation::Duplicate)
                if kind == ConsoleObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.console.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::ServiceOutputChannel, CapabilityOperation::Duplicate)
                if kind == ByteChannelObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(
                    self.service_output_channel
                        .as_ref()
                        .ok_or(LaunchError::UnsupportedAuthority)?
                        .as_handle_ref(),
                    purpose,
                    offer,
                )
            }
            (BootstrapAuthority::EnvironmentDirectory, CapabilityOperation::Duplicate)
                if kind == DirectoryObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(environment.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::BootEnvironmentDirectory, CapabilityOperation::Duplicate)
                if kind == DirectoryObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.boot_environment.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::RootDirectory, CapabilityOperation::Duplicate)
                if kind == DirectoryObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.root_directory.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::DynamicLibraryDirectory, CapabilityOperation::Duplicate)
                if kind == DirectoryObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.library_directory.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::TaskFactory, CapabilityOperation::Duplicate)
                if kind == TaskFactoryObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.factory.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::TaskGroup, CapabilityOperation::Duplicate)
                if kind == TaskGroupObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.group.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::ResourceDomain, CapabilityOperation::Duplicate)
                if kind == ResourceDomainObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.domain.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::TaskInspector, CapabilityOperation::Duplicate)
                if kind == TaskInspectorObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.task_inspector.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::ObjectInspector, CapabilityOperation::Duplicate)
                if kind == ObjectInspectorObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.object_inspector.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::MemoryInspector, CapabilityOperation::Duplicate)
                if kind == MemoryInspectorObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.memory_inspector.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::CpuInspector, CapabilityOperation::Duplicate)
                if kind == CpuInspectorObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.cpu_inspector.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::DeviceAuthority, CapabilityOperation::Duplicate)
                if kind == DeviceAssignmentAuthorityObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(
                    self.device
                        .as_ref()
                        .ok_or(LaunchError::UnsupportedAuthority)?
                        .as_handle_ref(),
                    purpose,
                    offer,
                )
            }
            (BootstrapAuthority::VmAuthority, CapabilityOperation::Duplicate)
                if kind == VirtualMachineCreationAuthorityObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(
                    self.vm_creation
                        .as_ref()
                        .ok_or(LaunchError::UnsupportedAuthority)?
                        .as_handle_ref(),
                    purpose,
                    offer,
                )
            }
            (authority, CapabilityOperation::Move) if kind == ByteChannelObject::KIND.as_raw() => {
                return self.move_channel_into_builder(authority, builder, purpose, rights);
            }
            (
                authority @ (BootstrapAuthority::ConsoleControlServer
                | BootstrapAuthority::ConsoleLogServer
                | BootstrapAuthority::IoBrokerServer
                | BootstrapAuthority::IoBrokerClient
                | BootstrapAuthority::FilesystemAttach
                | BootstrapAuthority::FilesystemProvider
                | BootstrapAuthority::FilesystemControlServer),
                CapabilityOperation::Move,
            ) if kind == CapabilityChannelObject::KIND.as_raw() => {
                let slot = match authority {
                    BootstrapAuthority::ConsoleControlServer => &mut self.raw_control_server,
                    BootstrapAuthority::ConsoleLogServer => &mut self.log_server,
                    BootstrapAuthority::IoBrokerServer => &mut self.io_broker_server,
                    BootstrapAuthority::IoBrokerClient => &mut self.io_broker_client,
                    BootstrapAuthority::FilesystemAttach => &mut self.filesystem_attach,
                    BootstrapAuthority::FilesystemProvider => &mut self.filesystem_provider,
                    BootstrapAuthority::FilesystemControlServer => {
                        &mut self.filesystem_control_server
                    }
                    _ => return Err(LaunchError::UnsupportedAuthority),
                };
                let handle = slot.take().ok_or(LaunchError::UnsupportedAuthority)?;
                return builder
                    .add_handle_move(handle, purpose, offer)
                    .map_err(|failure| {
                        let (error, handle) = failure.into_parts();
                        *slot = Some(handle);
                        LaunchError::service_resources(error)
                    });
            }
            (BootstrapAuthority::ConsoleController, CapabilityOperation::Duplicate)
                if kind == CapabilityChannelObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(
                    self.raw_controller
                        .as_ref()
                        .ok_or(LaunchError::UnsupportedAuthority)?
                        .as_handle_ref(),
                    purpose,
                    offer,
                )
            }
            (BootstrapAuthority::ConsoleLogConnector, CapabilityOperation::Duplicate)
                if kind == CapabilityChannelObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(self.log_connector.as_handle_ref(), purpose, offer)
            }
            (BootstrapAuthority::FilesystemControlClient, CapabilityOperation::Duplicate)
                if kind == CapabilityChannelObject::KIND.as_raw() =>
            {
                builder.add_handle_duplicate(
                    self.filesystem_control_client
                        .as_ref()
                        .ok_or(LaunchError::UnsupportedAuthority)?
                        .as_handle_ref(),
                    purpose,
                    offer,
                )
            }
            _ => return Err(LaunchError::UnsupportedAuthority),
        }
        .map_err(LaunchError::service_resources)
    }

    fn move_channel_into_builder(
        &mut self,
        authority: BootstrapAuthority,
        builder: &ProcessBuilder,
        purpose: u32,
        rights: Rights,
    ) -> Result<(), LaunchError> {
        let slot = match authority {
            BootstrapAuthority::IoReadyChannel => &mut self.io_ready_channel,
            _ => return Err(LaunchError::UnsupportedAuthority),
        };
        let channel = slot.take().ok_or(LaunchError::AuthorityConsumed)?;
        match builder.add_handle_move(channel, purpose, RightsOffer::Exact(rights)) {
            Ok(()) => Ok(()),
            Err(failure) => {
                let (error, channel) = failure.into_parts();
                *slot = Some(channel);
                Err(LaunchError::service_resources(error))
            }
        }
    }
}
