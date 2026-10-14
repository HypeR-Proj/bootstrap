// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Explicit command locations, separate from the shell's live working directory.

use crate::path::{CanonicalPath, Error};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageSource {
    Environment,
    Namespace,
}

pub struct Program {
    pub path: String,
    pub source: ImageSource,
    pub filesystem_authority: bool,
}

impl Program {
    pub fn resolve(name: &str, cwd: &CanonicalPath) -> Result<Self, Error> {
        let (source, path) = if name.contains('/') {
            (
                ImageSource::Namespace,
                cwd.resolve(name)?.as_str()?.to_owned(),
            )
        } else {
            (ImageSource::Environment, format!("bin/{name}"))
        };
        // Only recovery filesystem administration receives a service connector.
        let filesystem_authority = matches!(
            (source, path.as_str()),
            (ImageSource::Environment, "bin/mount" | "bin/umount")
                | (ImageSource::Namespace, "/bin/mount" | "/bin/umount")
        );
        Ok(Self {
            path,
            source,
            filesystem_authority,
        })
    }
}

#[cfg(test)]
#[path = "../tests/program.rs"]
mod tests;
