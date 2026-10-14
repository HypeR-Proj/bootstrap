// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn bare_commands_only_search_the_boot_environment() -> Result<(), Error> {
    let cwd = CanonicalPath::root().resolve("/data")?;
    for name in ["mount", "umount", "sh", "ls", "vmm", "fallback-sh"] {
        let program = Program::resolve(name, &cwd)?;
        assert_eq!(program.path, format!("bin/{name}"));
        assert_eq!(program.source, ImageSource::Environment);
        assert_eq!(
            program.filesystem_authority,
            matches!(name, "mount" | "umount")
        );
    }
    Ok(())
}

#[test]
fn explicit_paths_preserve_location_without_formal_authority() -> Result<(), Error> {
    let cwd = CanonicalPath::root().resolve("/data/tools")?;
    for (input, expected) in [
        ("./mount", "/data/tools/mount"),
        ("../vmm", "/data/vmm"),
        ("/system/bin/sh", "/system/bin/sh"),
        ("/system/bin/mount", "/system/bin/mount"),
    ] {
        let program = Program::resolve(input, &cwd)?;
        assert_eq!(program.path, expected);
        assert_eq!(program.source, ImageSource::Namespace);
        assert!(!program.filesystem_authority);
    }
    for name in ["/bin/mount", "/bin/umount"] {
        assert!(Program::resolve(name, &cwd)?.filesystem_authority);
    }
    Ok(())
}
