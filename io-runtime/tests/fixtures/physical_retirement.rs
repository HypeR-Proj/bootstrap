// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Destructive manual hardware fixture; excluded from production builds.

pub(super) fn arm() -> Result<(), String> {
    std::thread::Builder::new()
        .name("physical-retirement-probe".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(60));
            eprintln!("DMA-QUAL: terminating I/O runtime without cooperative device shutdown");
            // Process exit deliberately drops the owner while its Linux VM and
            // physical assignments are active. Kernel quarantine must retain
            // the VM, claims and all imported memory; no test may release them.
            std::process::exit(99);
        })
        .map_err(|error| format!("arm physical retirement probe: {error}"))?;
    println!("DMA-QUAL: armed; owner loss in 60 seconds; run storage stress now");
    Ok(())
}
