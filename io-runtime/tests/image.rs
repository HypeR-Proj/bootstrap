// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn admission_rejects_foreign_itb_architecture_with_diagnostic() {
    let config = Configuration {
        memory_bytes: 128 * 1024 * 1024,
        vcpus: 1,
        bootargs: String::new(),
        affinity: Vec::new(),
    };
    for (name, architecture) in [
        ("aarch64", Architecture::Aarch64),
        ("riscv64", Architecture::Riscv64),
        ("x86_64", Architecture::X86_64),
    ] {
        if name == std::env::consts::ARCH {
            continue;
        }
        let image = GuestImage {
            architecture,
            initramfs: None,
            kernel: hyper_vm_image::Payload {
                file_offset: 0,
                length: 64,
                load_address: 0,
                entry_address: 0,
                compression: hyper_vm_image::Compression::None,
            },
        };
        let error = configure(image, &config).unwrap_err();
        assert!(error.contains("VM architecture mismatch"), "{error}");
        assert!(error.contains(&format!("ITB {architecture:?}")), "{error}");
    }
}
