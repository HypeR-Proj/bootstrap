// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Prepare an independent system root before any successor process executes.
//! The kernel transition token makes pre-handoff rollback allocation-free.

use hyper_os::fs::{Directory, DirectoryRights};
use hyper_os::handle::{OwnedHandle, RootTransitionObject};

impl crate::profile::Handoff {
    /// Mountpoints are payload content, including on read-only EROFS. Stage1
    /// creates no directory or compatibility link in the system filesystem.
    pub(super) fn enter(
        &self,
        root: &Directory,
    ) -> hyper_os::Result<(OwnedHandle<RootTransitionObject>, u64)> {
        let rights = DirectoryRights::READ.union(DirectoryRights::INSPECT);
        let system = root.open_directory(&self.system_root, rights)?;
        let mount = system.info()?.location().mount();
        if mount == root.info()?.location().mount() {
            return Err(hyper_os::Error::InvalidResponse);
        }
        for path in [&*self.put_old, "/tmp"] {
            let directory = system.open_directory(path, rights)?;
            if directory.info()?.location().mount() != mount {
                return Err(hyper_os::Error::InvalidResponse);
            }
        }
        // Preparation changes only the reversible root view. Formal init
        // installs its writable /tmp after accepting supervisor ownership.
        let token = hyper_os::filesystem::begin_root_switch(
            root.as_handle_ref(),
            &self.system_root,
            &self.put_old,
            &[],
        )?;
        Ok((token, mount.get()))
    }
}
