// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Prepare an independent system root before any successor process executes.
//! The kernel transition token makes pre-handoff rollback allocation-free.

use hyper_os::fs::{Directory, DirectoryRights, MAX_PATH_BYTES};
use hyper_os::handle::{OwnedHandle, RootTransitionObject};
use serde::Deserialize;
use std::io::Read;

use super::Error;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct SystemInitConfig {
    pub(super) system_init: String,
    pub(super) system_root: String,
    pub(super) put_old: String,
}

impl SystemInitConfig {
    pub(super) fn load() -> Result<Option<Self>, Error> {
        let file = match std::fs::File::open("/etc/hyper/init.json") {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Source),
        };
        let mut text = String::new();
        file.take(4097)
            .read_to_string(&mut text)
            .map_err(|_| Error::Source)?;
        if text.len() > 4096 {
            return Err(Error::Source);
        }
        Self::parse(&text).map(Some)
    }

    fn parse(text: &str) -> Result<Self, Error> {
        let value: Self = serde_json::from_str(text).map_err(|_| Error::Source)?;
        if !absolute(&value.system_init)
            || !absolute(&value.system_root)
            || !absolute(&value.put_old)
            || value.system_root.len() + value.put_old.len() > MAX_PATH_BYTES
        {
            return Err(Error::Source);
        }
        Ok(value)
    }

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

fn absolute(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && path.len() <= MAX_PATH_BYTES
        && !path.as_bytes().contains(&0)
        && path[1..]
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_independent_root_and_successor_paths() {
        assert!(
            SystemInitConfig::parse(
                r#"{"system-root":"/system","put-old":"/boot","system-init":"/svc/init"}"#
            )
            .is_ok()
        );
        for root in ["/", "relative", "/x/../system", "//system", "/system/"] {
            let input =
                serde_json::json!({"system-root":root,"put-old":"/boot","system-init":"/svc/init"});
            assert!(SystemInitConfig::parse(&input.to_string()).is_err());
        }
        assert!(SystemInitConfig::parse(r#"{"system-init":"/svc/init"}"#).is_err());
    }
}
