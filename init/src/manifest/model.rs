// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_SERVICES: usize = 24;
pub const MAX_DEPENDENCIES_PER_SERVICE: usize = 12;
pub const MAX_DEPENDENCY_EDGES: usize = 128;
// Schema limits bound bootstrap resource use independently of heap storage.
// Core launchers carry independent namespace, image-environment and inspection
// roles; keep headroom below the Native startup-table limit for implicit entries.
pub const MAX_CAPABILITIES_PER_SERVICE: usize = 32;
pub const MAX_RIGHTS_PER_CAPABILITY: usize = 12;

pub(super) const MAX_SERVICE_NAME_BYTES: usize = 63;
pub(super) const MAX_IMAGE_PATH_BYTES: usize = 255;
pub(super) const MAX_BINDING_NAME_BYTES: usize = 95;
pub(super) const MAX_PURPOSE_NAME_BYTES: usize = 95;
pub(super) const MAX_RIGHT_NAME_BYTES: usize = 47;

/// Acquisition operation requested for one startup capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityOperation {
    Move,
    Duplicate,
    Create,
}

/// Supervision policy applied after a service process terminates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
}

/// One explicitly attenuated startup-capability binding.
#[derive(Debug, Eq, PartialEq)]
pub struct CapabilityBinding<'manifest> {
    pub(super) source: &'manifest str,
    pub(super) purpose: &'manifest str,
    pub(super) operation: CapabilityOperation,
    pub(super) rights: BoundedList<&'manifest str, MAX_RIGHTS_PER_CAPABILITY>,
}

impl CapabilityBinding<'_> {
    pub const fn source(&self) -> &str {
        self.source
    }

    pub const fn purpose(&self) -> &str {
        self.purpose
    }

    pub const fn operation(&self) -> CapabilityOperation {
        self.operation
    }

    pub fn rights(&self) -> impl Iterator<Item = &str> {
        self.rights.iter().copied()
    }
}

/// One service declaration in manifest order.
#[derive(Debug, Eq, PartialEq)]
pub struct Service<'manifest> {
    pub(super) name: &'manifest str,
    pub(super) image: &'manifest str,
    pub(super) critical: bool,
    pub(super) restart: RestartPolicy,
    pub(super) dependencies: BoundedList<&'manifest str, MAX_DEPENDENCIES_PER_SERVICE>,
    pub(super) capabilities:
        BoundedList<CapabilityBinding<'manifest>, MAX_CAPABILITIES_PER_SERVICE>,
}

impl Service<'_> {
    pub const fn name(&self) -> &str {
        self.name
    }

    pub const fn image(&self) -> &str {
        self.image
    }

    pub const fn critical(&self) -> bool {
        self.critical
    }

    pub const fn restart(&self) -> RestartPolicy {
        self.restart
    }

    pub fn dependencies(&self) -> impl Iterator<Item = &str> {
        self.dependencies.iter().copied()
    }

    pub fn capabilities(&self) -> impl Iterator<Item = &CapabilityBinding<'_>> {
        self.capabilities.iter()
    }
}

/// A parsed manifest which borrows immutable text loaded from the root directory.
#[derive(Debug, Eq, PartialEq)]
pub struct Manifest<'manifest> {
    pub(super) services: BoundedList<Service<'manifest>, MAX_SERVICES>,
}

impl<'manifest> Manifest<'manifest> {
    pub(crate) fn empty() -> Self {
        Self {
            services: BoundedList::new(),
        }
    }

    pub fn services(&self) -> impl Iterator<Item = &Service<'_>> {
        self.services.iter()
    }

    pub const fn service_count(&self) -> usize {
        self.services.len()
    }

    pub fn service(&self, index: usize) -> Option<&Service<'_>> {
        self.services.get(index)
    }
}

/// A schema limit over ordinary Vec storage, not a second container implementation.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct BoundedList<T, const N: usize>(Vec<T>);

impl<T, const N: usize> BoundedList<T, N> {
    pub(super) fn new() -> Self {
        Self(Vec::new())
    }
    pub(super) fn push(&mut self, value: T) -> Result<(), T> {
        if self.0.len() == N {
            return Err(value);
        }
        self.0.push(value);
        Ok(())
    }
    pub(super) const fn len(&self) -> usize {
        self.0.len()
    }
    pub(super) const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(super) fn get(&self, index: usize) -> Option<&T> {
        self.0.get(index)
    }
    pub(super) fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }
}

impl<T, const N: usize> IntoIterator for BoundedList<T, N> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
