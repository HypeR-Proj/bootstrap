// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! QEMU acceptance executable, excluded from ordinary application builds.

use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(target_os = "hyper")]
use std::os::hyper::fs::MetadataExt;
use std::process::ExitCode;
use std::time::{Duration, Instant, UNIX_EPOCH};

const DIRECTORY: &str = "/data/acceptance";
const PENDING: &str = "/data/acceptance/pending.bin";
const PAYLOAD: &str = "/data/acceptance/persistent 数据.bin";
const COPY: &str = "/data/acceptance/copy.bin";
const COHERENCE: &str = "/data/acceptance/cache-coherence.bin";
const COHERENCE_ALIAS: &str = "/data/acceptance/CACHE-COHERENCE.BIN";
#[cfg(not(feature = "userspace-device-test"))]
const LENGTH: usize = 3 * 1024 * 1024 + 37;
// Still exceeds the Native block transfer window and crosses cluster boundaries;
// this fixture targets the physical userspace IRQ/MMIO path, not FAT capacity.
#[cfg(feature = "userspace-device-test")]
const LENGTH: usize = 256 * 1024 + 37;
const GAP: usize = 4096;
const OVERWRITE_BYTES: usize = 4096;
const OVERWRITE_OFFSETS: [usize; 4] = [
    17,
    128 * 1024 - 7,
    LENGTH / 2 + 511,
    LENGTH - OVERWRITE_BYTES,
];

fn payload_byte(offset: usize) -> u8 {
    if OVERWRITE_OFFSETS
        .iter()
        .any(|start| (*start..*start + OVERWRITE_BYTES).contains(&offset))
    {
        ((offset * 13 + 71) % 251) as u8
    } else {
        (offset % 251) as u8
    }
}

fn verify(path: &str) -> io::Result<()> {
    println!("BOARD-STORAGE: opening {path}");
    let mut file = File::open(path)?;
    println!("BOARD-STORAGE: checking metadata {path}");
    if file.metadata()?.len() != (LENGTH + GAP + 1) as u64 {
        return Err(io::Error::other("incorrect persisted file length"));
    }
    // Cross all four request queues, including unaligned heads and tails.
    let mut buffer = vec![0; 512 * 1024];
    println!("BOARD-STORAGE: reading {path}");
    let mut offset = 0;
    while offset < LENGTH {
        let count = if offset == 0 {
            13
        } else {
            buffer.len().min(LENGTH - offset)
        };
        file.read_exact(&mut buffer[..count])?;
        if buffer[..count]
            .iter()
            .enumerate()
            .any(|(i, byte)| *byte != payload_byte(offset + i))
        {
            return Err(io::Error::other("persisted data mismatch"));
        }
        offset += count;
    }
    file.read_exact(&mut buffer[..GAP + 1])?;
    if buffer[..GAP].iter().any(|byte| *byte != 0) || buffer[GAP] != 0xa5 {
        return Err(io::Error::other("sparse extension mismatch"));
    }
    if file.read(&mut buffer[..1])? != 0 {
        return Err(io::Error::other("read passed end of file"));
    }
    Ok(())
}

fn verify_handle(file: &mut File, expected: &[u8]) -> io::Result<()> {
    if file.metadata()?.len() != expected.len() as u64 {
        return Err(io::Error::other("retained handle has stale length"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut output = vec![0xa7; expected.len() + 1];
    if file.read(&mut output)? != expected.len()
        || &output[..expected.len()] != expected
        || output[expected.len()] != 0xa7
    {
        return Err(io::Error::other(
            "retained handle has stale contents or EOF",
        ));
    }
    Ok(())
}

fn concurrent_contents() -> io::Result<()> {
    use std::sync::Barrier;

    let mut writer = OpenOptions::new().write(true).open(COHERENCE)?;
    let mut reader = File::open(COHERENCE_ALIAS)?;
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        let writing = scope.spawn(|| {
            let mut result = Ok(());
            let mut bytes = vec![0; 8192];
            for round in 0..16 {
                barrier.wait();
                if result.is_ok() {
                    bytes.fill(0xa0 + round);
                    result = (|| {
                        writer.seek(SeekFrom::Start(0))?;
                        if writer.write(&bytes)? != bytes.len() {
                            return Err(io::Error::other("short concurrent write"));
                        }
                        Ok(())
                    })();
                }
                // Complete both rendezvous even after an I/O error so the
                // other thread can report its result and join normally.
                barrier.wait();
            }
            result
        });
        let mut result = Ok(());
        let mut bytes = vec![0; 8192];
        for round in 0..16 {
            barrier.wait();
            if result.is_ok() {
                result = (|| {
                    reader.seek(SeekFrom::Start(0))?;
                    if reader.read(&mut bytes)? != bytes.len()
                        || bytes.iter().any(|byte| *byte != bytes[0])
                    {
                        return Err(io::Error::other("read mixed concurrent content revisions"));
                    }
                    Ok(())
                })();
            }
            barrier.wait();
            if result.is_ok() {
                // The writer is now waiting for the next round. This read
                // must see its completed write, even if the racing read hit
                // an older page or started a fill before the mutation.
                bytes.fill(0xa0 + round);
                result = verify_handle(&mut reader, &bytes);
            }
        }
        let written = writing
            .join()
            .map_err(|_| io::Error::other("concurrent writer panicked"))?;
        result.and(written)
    })
}

fn cache_transitions() -> io::Result<()> {
    println!("BOARD-STORAGE: creating coherence file");
    let mut writer = File::create(COHERENCE)?;
    let mut expected = vec![0x31; 8192 + 37];
    writer.write_all(&expected)?;
    // FAT case aliases must resolve to the same content owner as separate
    // opens with the stored spelling. Full reads warm all affected pages.
    let mut reader = File::open(COHERENCE_ALIAS)?;
    verify_handle(&mut reader, &expected)?;
    println!("BOARD-STORAGE: coherence pages warmed");
    writer.set_len(4093)?;
    expected.truncate(4093);
    verify_handle(&mut reader, &expected)?;
    println!("BOARD-STORAGE: coherence shrink verified");
    writer.set_len(8192 + 19)?;
    expected.resize(8192 + 19, 0);
    verify_handle(&mut reader, &expected)?;
    println!("BOARD-STORAGE: coherence extension verified");

    let truncated = File::create(COHERENCE_ALIAS)?;
    verify_handle(&mut reader, &[])?;
    println!("BOARD-STORAGE: coherence truncate verified");
    drop(truncated);
    let mut append = OpenOptions::new().append(true).open(COHERENCE)?;
    expected.resize(8192, 0x66);
    expected.fill(0x66);
    append.write_all(&expected)?;
    verify_handle(&mut reader, &expected)?;
    println!("BOARD-STORAGE: coherence append verified");
    drop(append);
    drop(writer);
    drop(reader);
    println!("BOARD-STORAGE: starting concurrent coherence reads and writes");
    concurrent_contents()?;
    println!("BOARD-STORAGE: concurrent coherence verified; removing file");
    remove_closed_file(COHERENCE)?;
    println!("BOARD-STORAGE: shared content coherence PASS");
    Ok(())
}

fn remove_closed_file(path: &str) -> io::Result<()> {
    // Closing handles retires their authority synchronously, but FileObject
    // destruction runs on the kernel reaper. VFS rejects live node leases
    // until then. Cached pages must not keep that lease alive indefinitely.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::remove_file(path) {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::ResourceBusy => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("closed file retained a node lease ({path}): {error}"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(target_os = "hyper")]
fn cached_identity_lifetime() -> io::Result<()> {
    const BEFORE: &str = "/data/acceptance/cache-identity-before";
    const AFTER: &str = "/data/acceptance/cache-identity-after";
    const OLD_FILE: &str = "/data/acceptance/cache-identity-before/identity.bin";
    const OLD_ALIAS: &str = "/data/acceptance/cache-identity-before/IDENTITY.BIN";
    const NEW_FILE: &str = "/data/acceptance/cache-identity-after/identity.bin";
    for path in [BEFORE, AFTER] {
        match fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::create_dir(BEFORE)?;
    let bytes = vec![0x3d; 8192];
    let identity = {
        let mut file = File::create(OLD_FILE)?;
        file.write_all(&bytes)?;
        let mut reader = File::open(OLD_FILE)?;
        verify_handle(&mut reader, &bytes)?;
        reader.metadata()?.identity()
    };
    // Native handle retirement is synchronous, but final FileObject teardown
    // runs on the reaper. Yield a scheduling interval before the new opens so
    // this checks page-owned identity retention after ordinary handle closure.
    std::thread::sleep(Duration::from_millis(50));
    for path in [OLD_FILE, OLD_ALIAS] {
        let mut reopened = File::open(path)?;
        if reopened.metadata()?.identity() != identity {
            return Err(io::Error::other(
                "cached identity changed across close and reopen",
            ));
        }
        verify_handle(&mut reopened, &bytes)?;
    }
    std::thread::sleep(Duration::from_millis(50));
    // Parent rename must update idle descendant bindings as well as open ones.
    fs::rename(BEFORE, AFTER)?;
    {
        let mut renamed = File::open(NEW_FILE)?;
        if renamed.metadata()?.identity() != identity {
            return Err(io::Error::other(
                "cached identity changed after parent rename",
            ));
        }
        verify_handle(&mut renamed, &bytes)?;
    }
    remove_closed_file(NEW_FILE)?;
    let replacement = vec![0x79; 8192];
    {
        let mut writer = File::create(NEW_FILE)?;
        if writer.metadata()?.identity() == identity {
            return Err(io::Error::other(
                "recreated file reused a retired content identity",
            ));
        }
        writer.write_all(&replacement)?;
        let mut reader = File::open(NEW_FILE)?;
        verify_handle(&mut reader, &replacement)?;
    }
    remove_closed_file(NEW_FILE)?;
    fs::remove_dir(AFTER)?;
    println!("BOARD-STORAGE: cached identity reopen/rename/recreate PASS");
    Ok(())
}

fn run(mode: &str) -> io::Result<()> {
    println!("BOARD-STORAGE: {mode} started");
    if mode == "write" {
        fs::create_dir_all(DIRECTORY)?;
        for path in [PENDING, PAYLOAD, COPY, COHERENCE] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let mut file = File::create(PENDING)?;
        println!("BOARD-STORAGE: writing payload");
        let mut buffer = vec![0; 512 * 1024];
        let mut offset = 0;
        while offset < LENGTH {
            let count = buffer.len().min(LENGTH - offset);
            for (i, byte) in buffer[..count].iter_mut().enumerate() {
                *byte = ((offset + i) % 251) as u8;
            }
            // A healthy backing device accepts this bounded batch in one
            // syscall; catch accidental reintroduction of the 1 KiB cap.
            if file.write(&buffer[..count])? != count {
                return Err(io::Error::other("unexpected short bulk write"));
            }
            offset += count;
        }
        let mut reader = File::open(PENDING)?;
        // Reopen the FAT cursor at unrelated offsets, including sector/window
        // boundaries. Different bytes expose stale seek mappings or lost writes
        // both immediately and after the cold restart.
        for offset in OVERWRITE_OFFSETS.into_iter().rev() {
            let page_start = offset / 4096 * 4096;
            let warm_bytes = (LENGTH - page_start).min(8192);
            reader.seek(SeekFrom::Start(page_start as u64))?;
            reader.read_exact(&mut buffer[..warm_bytes])?;
            for (i, byte) in buffer[..OVERWRITE_BYTES].iter_mut().enumerate() {
                *byte = payload_byte(offset + i);
            }
            file.seek(SeekFrom::Start(offset as u64))?;
            file.write_all(&buffer[..OVERWRITE_BYTES])?;
            reader.seek(SeekFrom::Start(offset as u64))?;
            reader.read_exact(&mut buffer[..OVERWRITE_BYTES])?;
            if buffer[..OVERWRITE_BYTES]
                .iter()
                .enumerate()
                .any(|(i, byte)| *byte != payload_byte(offset + i))
            {
                return Err(io::Error::other(
                    "cached page survived another handle's write",
                ));
            }
        }
        file.seek(SeekFrom::Start((LENGTH + GAP) as u64))?;
        file.write_all(&[0xa5])?;
        if reader.metadata()?.len() != (LENGTH + GAP + 1) as u64 {
            return Err(io::Error::other("cached length survived sparse extension"));
        }
        file.set_times(
            FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_750_000_000)),
        )?;
        file.sync_all()?;
        println!("BOARD-STORAGE: payload synchronized");
        drop(file);
        println!("BOARD-STORAGE: renaming payload");
        fs::rename(PENDING, PAYLOAD)?;
        println!("BOARD-STORAGE: payload renamed");
        reader.seek(SeekFrom::Start((LENGTH + GAP) as u64))?;
        reader.read_exact(&mut buffer[..1])?;
        if buffer[0] != 0xa5 {
            return Err(io::Error::other(
                "retained read handle lost renamed contents",
            ));
        }
        drop(reader);
        println!("BOARD-STORAGE: renamed handle verified; copying payload");
        fs::copy(PAYLOAD, COPY)?;
        println!("BOARD-STORAGE: payload copied");
        cache_transitions()?;
        #[cfg(target_os = "hyper")]
        cached_identity_lifetime()?;
        // The second fence includes the rename and copy's directory entries.
        OpenOptions::new().write(true).open(COPY)?.sync_all()?;
        println!("BOARD-STORAGE: copy synchronized");
    } else if mode != "verify" {
        return Err(io::Error::other("expected write or verify"));
    }
    verify(PAYLOAD)?;
    verify(COPY)?;
    if fs::metadata(PAYLOAD)?.modified()? != UNIX_EPOCH + Duration::from_secs(1_750_000_000) {
        return Err(io::Error::other("persisted modification time mismatch"));
    }
    if fs::read_dir(DIRECTORY)?.try_fold(0, |count, entry| entry.map(|_| count + 1))? != 2 {
        return Err(io::Error::other("directory entries mismatch"));
    }
    Ok(())
}

fn main() -> ExitCode {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match run(&mode) {
        Ok(()) => {
            println!("BOARD-STORAGE: {mode} PASS");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("BOARD-STORAGE: {mode} FAIL: {error}");
            ExitCode::FAILURE
        }
    }
}
