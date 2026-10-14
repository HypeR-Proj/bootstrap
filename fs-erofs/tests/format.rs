// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use crate::{Device, Error, Kind, Volume};
use std::prelude::v1::*;
use std::vec;

struct Memory(Vec<u8>);
impl Device for Memory {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::Io)?;
        let end = start.checked_add(output.len()).ok_or(Error::Io)?;
        output.copy_from_slice(self.0.get(start..end).ok_or(Error::Io)?);
        Ok(())
    }
}

fn word(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn dword(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn qword(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn inode(
    bytes: &mut [u8],
    nid: usize,
    mode: u16,
    size: u64,
    extended: bool,
    inline: bool,
    block: u32,
) -> usize {
    let offset = nid * 32;
    word(
        bytes,
        offset,
        u16::from(extended) | if inline { 4 } else { 0 },
    );
    word(bytes, offset + 4, mode);
    if extended {
        qword(bytes, offset + 8, size);
        qword(bytes, offset + 32, 123);
        dword(bytes, offset + 40, 456);
        dword(bytes, offset + 44, 1);
    } else {
        word(bytes, offset + 6, 1);
        dword(bytes, offset + 8, size as u32);
    }
    dword(bytes, offset + 16, block);
    offset + if extended { 64 } else { 32 }
}

fn directory(bytes: &mut [u8], entries: &[(u64, u8, &str)]) -> usize {
    let mut end = entries.len() * 12;
    for (index, &(nid, kind, name)) in entries.iter().enumerate() {
        qword(bytes, index * 12, nid);
        word(bytes, index * 12 + 8, end as u16);
        bytes[index * 12 + 10] = kind;
        bytes[end..end + name.len()].copy_from_slice(name.as_bytes());
        end += name.len();
    }
    end
}

fn fixture() -> Vec<u8> {
    let mut bytes = vec![0; 6 * 4096];
    dword(&mut bytes, 1024, 0xe0f5_e1e2);
    dword(&mut bytes, 1032, 2);
    bytes[1036] = 12;
    word(&mut bytes, 1038, 40);
    qword(&mut bytes, 1040, 6);
    qword(&mut bytes, 1048, 100);
    dword(&mut bytes, 1060, 6);
    let entries = [
        (40, 2, "."),
        (40, 2, ".."),
        (64, 1, "big"),
        (80, 2, "dir"),
        (96, 7, "link"),
        (112, 1, "small"),
    ];
    let length = directory(&mut bytes[1312..], &entries);
    inode(&mut bytes, 40, 0o40755, length as u64, false, true, 0);
    inode(&mut bytes, 64, 0o100755, 8193, true, false, 2);
    for index in 0..8193 {
        bytes[8192 + index] = (index % 251) as u8;
    }
    let length = directory(
        &mut bytes[2592..],
        &[(80, 2, "."), (40, 2, ".."), (112, 1, "child")],
    );
    inode(&mut bytes, 80, 0o40755, length as u64, false, true, 0);
    let target = b"../dir/child";
    let start = inode(&mut bytes, 96, 0o120777, target.len() as u64, true, true, 0);
    bytes[start..start + target.len()].copy_from_slice(target);
    let start = inode(&mut bytes, 112, 0o100644, 5, false, true, 0);
    bytes[start..start + 5].copy_from_slice(b"hello");
    bytes
}

#[test]
fn compact_extended_inline_plain_and_nested_paths() -> Result<(), Error> {
    let mut volume = Volume::mount(Memory(fixture()))?;
    assert_eq!(volume.stat("")?.kind, Kind::Directory);
    assert_eq!(volume.stat("big")?.modified, (123, 456));
    assert_eq!(volume.stat("small")?.modified, (100, 0));
    let mut output = [0; 12];
    assert_eq!(volume.read("dir/child", 0, &mut output)?, 5);
    assert_eq!(&output[..5], b"hello");
    let mut big = vec![0; 8200];
    assert_eq!(volume.read("big", 0, &mut big)?, 8193);
    assert!(
        big[..8193]
            .iter()
            .enumerate()
            .all(|(i, v)| *v == (i % 251) as u8)
    );
    assert_eq!(volume.read("big", 4093, &mut output)?, 12);
    assert!(
        output
            .iter()
            .enumerate()
            .all(|(i, v)| *v == ((4093 + i) % 251) as u8)
    );
    assert_eq!(volume.read("big", u64::MAX, &mut output)?, 0);
    assert_eq!(
        directory_entry(&mut volume, "", 0)?
            .ok_or(Error::Missing)?
            .name,
        "big"
    );
    assert_eq!(
        directory_entry(&mut volume, "", 3)?
            .ok_or(Error::Missing)?
            .name,
        "small"
    );
    assert!(directory_entry(&mut volume, "", 4)?.is_none());
    assert!(directory_entry(&mut volume, "", usize::MAX)?.is_none());
    Ok(())
}

#[test]
fn symlinks_remain_namespace_objects() -> Result<(), Error> {
    let mut volume = Volume::mount(Memory(fixture()))?;
    let mut output = [0; 64];
    assert_eq!(volume.stat("link")?.kind, Kind::Symlink);
    let count = volume.readlink("link", &mut output)?;
    assert_eq!(&output[..count], b"../dir/child");
    assert_eq!(
        volume.read("link", 0, &mut output),
        Err(Error::InvalidInput)
    );
    assert_eq!(
        volume.readlink("small", &mut output),
        Err(Error::InvalidInput)
    );
    assert!(volume.stat("link/child").is_err());
    for path in ["/small", "dir//child", "dir/../small", "./small", "small/"] {
        assert!(volume.stat(path).is_err(), "{path}");
    }
    Ok(())
}

#[test]
fn mount_rejects_unsupported_features_and_corrupt_root() {
    for (offset, value) in [
        (1036, 8),
        (1037, 1),
        (1104, 1),
        (1108, 1),
        (1114, 1),
        (1282, 1),
    ] {
        let mut bytes = fixture();
        bytes[offset] = value;
        assert!(
            matches!(Volume::mount(Memory(bytes)), Err(Error::Unsupported)),
            "{offset}"
        );
    }
    let mut bytes = fixture();
    word(&mut bytes, 1284, 0o100644);
    assert!(matches!(Volume::mount(Memory(bytes)), Err(Error::Corrupt)));
}

#[test]
fn malformed_directory_offsets_and_names_fail_before_lookup() {
    for offset in [1312 + 8, 1312 + 12 + 8] {
        let mut bytes = fixture();
        word(&mut bytes, offset, 0xffff);
        assert!(Volume::mount(Memory(bytes)).is_err());
    }
    let mut bytes = fixture();
    // First entry's name must not overlap its directory entry table.
    word(&mut bytes, 1320, 1);
    assert!(Volume::mount(Memory(bytes)).is_err());
    let mut bytes = fixture();
    bytes[1312 + 72] = b'/';
    assert!(Volume::mount(Memory(bytes)).is_err());
}

#[test]
fn inode_ranges_and_directory_type_hints_are_checked() -> Result<(), Error> {
    for bad_nid in [u64::MAX, 9000] {
        let mut bytes = fixture();
        qword(&mut bytes, 1312 + 24, bad_nid);
        let mut volume = Volume::mount(Memory(bytes))?;
        assert!(matches!(volume.stat("big"), Err(Error::Corrupt)));
    }
    let mut bytes = fixture();
    dword(&mut bytes, 64 * 32 + 16, u32::MAX);
    assert!(Volume::mount(Memory(bytes))?.stat("big").is_err());
    let mut bytes = fixture();
    bytes[1312 + 24 + 10] = 2;
    assert!(matches!(
        Volume::mount(Memory(bytes))?.stat("big"),
        Err(Error::Corrupt)
    ));
    Ok(())
}

#[test]
fn truncated_media_never_panics() {
    for length in [0, 1024, 1100, 1300, 2048, 4096, 8192, 10000] {
        let mut bytes = fixture();
        bytes.truncate(length);
        if let Ok(mut volume) = Volume::mount(Memory(bytes)) {
            assert!(volume.stat("big").is_err());
        }
    }
}

#[test]
fn inline_tail_cannot_cross_its_metadata_block() -> Result<(), Error> {
    let mut bytes = fixture();
    word(&mut bytes, 64 * 32, 5);
    qword(&mut bytes, 64 * 32 + 8, 4095);
    assert!(matches!(
        Volume::mount(Memory(bytes))?.stat("big"),
        Err(Error::Corrupt)
    ));
    Ok(())
}

#[test]
fn supported_block_sizes_and_split_inline_reads() -> Result<(), Error> {
    for bits in 9..=12 {
        let mut bytes = fixture();
        bytes[1036] = bits;
        dword(&mut bytes, 64 * 32 + 16, 8192 >> bits);
        // The final byte is inline after this extended inode; the preceding
        // file blocks still come from the external data extent.
        word(&mut bytes, 64 * 32, 5);
        bytes[64 * 32 + 64] = 0xa7;
        let mut volume = Volume::mount(Memory(bytes))?;
        let mut tail = [0; 4];
        assert_eq!(volume.read("big", 8190, &mut tail)?, 3);
        assert_eq!(&tail[..3], &[(8190 % 251) as u8, (8191 % 251) as u8, 0xa7]);
    }
    Ok(())
}

#[test]
fn core_format_ignores_extension_only_fields_and_directory_padding() -> Result<(), Error> {
    let mut bytes = fixture();
    word(&mut bytes, 1280, 4 | 0x10);
    dword(&mut bytes, 1280 + 12, u32::MAX);
    // Name data ends at byte 90; only the first NUL ends the final name.
    dword(&mut bytes, 1280 + 8, 95);
    bytes[1312 + 91..1312 + 95].fill(0xff);
    let mut volume = Volume::mount(Memory(bytes))?;
    assert_eq!(volume.stat("small")?.size, 5);
    Ok(())
}

#[test]
fn deterministic_metadata_mutations_remain_bounded() {
    let original = fixture();
    let mut state = 0x481a_9ef3u32;
    for _ in 0..1200 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let mut bytes = original.clone();
        let position = 1024 + state as usize % (4096 - 1024);
        bytes[position] ^= (state >> 16) as u8;
        if let Ok(mut volume) = Volume::mount(Memory(bytes)) {
            let mut buffer = [0; 17];
            let _ = volume.stat("big");
            let _ = volume.read("small", 0, &mut buffer);
            let _ = volume.readlink("link", &mut buffer);
            let _ = directory_entry(&mut volume, "", 7);
        }
    }
}

#[test]
fn official_mkfs_checksum_and_core_fixture() -> Result<(), Error> {
    let bytes = include_bytes!("fixtures/core.erofs").to_vec();
    let mut volume = Volume::mount(Memory(bytes.clone()))?;
    let mut output = [0; 64];
    let length = volume.read("hello", 0, &mut output)?;
    assert_eq!(&output[..length], b"EROFS reference fixture\n");
    let length = volume.readlink("link", &mut output)?;
    assert_eq!(&output[..length], b"hello");
    let mut corrupt = bytes;
    corrupt[1090] ^= 1; // Volume-label byte remains structurally valid.
    assert!(matches!(
        Volume::mount(Memory(corrupt)),
        Err(Error::Corrupt)
    ));
    Ok(())
}

fn large_directory_fixture(count: usize) -> Vec<u8> {
    let directory_blocks = count.div_ceil(64);
    let blocks = 8 + directory_blocks;
    let mut bytes = vec![0; blocks * 4096];
    dword(&mut bytes, 1024, 0xe0f5_e1e2);
    bytes[1036] = 12;
    word(&mut bytes, 1038, 40);
    qword(&mut bytes, 1040, 2);
    dword(&mut bytes, 1060, blocks as u32);
    inode(
        &mut bytes,
        40,
        0o40755,
        (directory_blocks * 4096) as u64,
        false,
        false,
        8,
    );
    inode(&mut bytes, 64, 0o100644, 0, false, false, 0);
    for block in 0..directory_blocks {
        let names: Vec<String> = (block * 64..count.min((block + 1) * 64))
            .map(|index| format!("F{index:07}"))
            .collect();
        let entries: Vec<_> = names.iter().map(|name| (64, 1, name.as_str())).collect();
        directory(&mut bytes[(8 + block) * 4096..(9 + block) * 4096], &entries);
    }
    bytes
}

struct CountedMemory {
    memory: Memory,
    reads: std::rc::Rc<std::cell::Cell<usize>>,
}
impl Device for CountedMemory {
    fn len(&self) -> u64 {
        self.memory.len()
    }
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
        self.reads.set(self.reads.get() + 1);
        self.memory.read_exact_at(offset, output)
    }
}

fn scan_large_erofs_directory(count: usize) -> Result<usize, Error> {
    use hyper_filesystem::protocol::{DIRECTORY_BATCH_BYTES, DirectoryBatch, DirectoryCursor};
    let reads = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut volume = Volume::mount(CountedMemory {
        memory: Memory(large_directory_fixture(count)),
        reads: reads.clone(),
    })?;
    let mut buffers = [[0; DIRECTORY_BATCH_BYTES]; 2];
    let mut cursors = [Some(DirectoryCursor::START); 2];
    let mut seen = [0; 2];
    let before = reads.get();
    while cursors.iter().any(Option::is_some) {
        for reader in 0..2 {
            let Some(cursor) = cursors[reader] else {
                continue;
            };
            let length = volume.read_directory("", cursor, &mut buffers[reader])?;
            let batch = DirectoryBatch::decode(&buffers[reader][..length]).ok_or(Error::Corrupt)?;
            assert_eq!(batch.start_cursor(), cursor);
            for record in batch.records() {
                assert_eq!(record.name(), format!("F{:07}", seen[reader]));
                seen[reader] += 1;
                cursors[reader] = Some(record.next_cursor());
            }
            if batch.eof() {
                cursors[reader] = None;
            }
        }
    }
    assert_eq!(seen, [count; 2]);
    Ok(reads.get() - before)
}

#[test]
fn directory_batches_resume_independent_readers_with_linear_io() -> Result<(), Error> {
    let small = scan_large_erofs_directory(1024)?;
    let large = scan_large_erofs_directory(4096)?;
    assert!(
        large <= small * 5,
        "directory I/O grew superlinearly: {small} -> {large}"
    );
    assert!(large < 1600, "too many EROFS directory reads: {large}");
    Ok(())
}

#[test]
fn directory_checkpoint_replay_and_cross_directory_rejection() -> Result<(), Error> {
    use hyper_filesystem::protocol::{DIRECTORY_MIN_BYTES, DirectoryBatch, DirectoryCursor};
    let mut volume = Volume::mount(Memory(large_directory_fixture(64)))?;
    let mut bytes = [0; DIRECTORY_MIN_BYTES];
    let length = volume.read_directory("", DirectoryCursor::START, &mut bytes)?;
    let batch = DirectoryBatch::decode(&bytes[..length]).ok_or(Error::Corrupt)?;
    assert!(!batch.eof());
    let cursor = batch.records().last().ok_or(Error::Corrupt)?.next_cursor();
    assert_eq!(
        volume.read_directory("other", cursor, &mut bytes),
        Err(Error::InvalidInput)
    );
    let length = volume.read_directory("", cursor, &mut bytes)?;
    let copy = bytes[..length].to_vec();
    assert_eq!(volume.read_directory("", cursor, &mut bytes)?, length);
    assert_eq!(&bytes[..length], copy);
    Ok(())
}

#[test]
fn directory_batches_resume_inside_inline_tail_and_finish_at_exact_eof() -> Result<(), Error> {
    use hyper_filesystem::protocol::{DIRECTORY_MIN_BYTES, DirectoryBatch, DirectoryCursor};
    let mut bytes = large_directory_fixture(96);
    // Keep 64 names in the external block, then put the final 32 names after
    // the compact root inode. This tail ends between filesystem blocks.
    let tail_length = 32 * (12 + 8);
    let inline = inode(
        &mut bytes,
        40,
        0o40755,
        (4096 + tail_length) as u64,
        false,
        true,
        8,
    );
    bytes.copy_within(9 * 4096..9 * 4096 + tail_length, inline);
    let mut volume = Volume::mount(Memory(bytes))?;
    let mut output = [0; DIRECTORY_MIN_BYTES];
    let mut cursor = DirectoryCursor::START;
    let mut seen = 0;
    let mut tail_batches = 0;
    loop {
        let length = volume.read_directory("", cursor, &mut output)?;
        let batch = DirectoryBatch::decode(&output[..length]).ok_or(Error::Corrupt)?;
        assert_eq!(batch.start_cursor(), cursor);
        for record in batch.records() {
            assert_eq!(record.name(), format!("F{seen:07}"));
            seen += 1;
            cursor = record.next_cursor();
        }
        if cursor.words[2] as u32 == 4096 {
            tail_batches += 1;
        }
        if batch.eof() {
            break;
        }
    }
    assert_eq!(seen, 96);
    assert!(tail_batches > 1, "inline tail did not require continuation");
    // The last entry leaves a valid position within the inline block. Resuming
    // that exact position returns empty EOF rather than seeking past its tail.
    let length = volume.read_directory("", cursor, &mut output)?;
    let batch = DirectoryBatch::decode(&output[..length]).ok_or(Error::Corrupt)?;
    assert!(batch.is_empty() && batch.eof());
    assert_eq!(batch.start_cursor(), cursor);
    Ok(())
}

fn directory_entry(
    volume: &mut Volume<Memory>,
    path: &str,
    mut index: usize,
) -> Result<Option<crate::Entry>, Error> {
    use hyper_filesystem::protocol::{
        self, DIRECTORY_BATCH_BYTES, DirectoryBatch, DirectoryCursor, Entry,
    };
    let mut bytes = [0; DIRECTORY_BATCH_BYTES];
    let mut cursor = DirectoryCursor::START;
    loop {
        let length = volume.read_directory(path, cursor, &mut bytes)?;
        let batch = DirectoryBatch::decode(&bytes[..length]).ok_or(Error::Corrupt)?;
        for record in batch.records() {
            if index == 0 {
                let mut entry = Entry::empty();
                record.entry_into(&mut entry);
                return Ok(Some(crate::Entry {
                    name: entry.name().to_owned(),
                    kind: match entry.kind {
                        protocol::FILE => Kind::File,
                        protocol::DIRECTORY => Kind::Directory,
                        _ => Kind::Symlink,
                    },
                    mode: entry.mode,
                    size: entry.size,
                    modified: entry
                        .modified
                        .map(|time| (time.seconds(), time.nanoseconds()))
                        .ok_or(Error::Corrupt)?,
                }));
            }
            index -= 1;
            cursor = record.next_cursor();
        }
        if batch.eof() {
            return Ok(None);
        }
    }
}

#[test]
fn provider_reopen_preserves_directory_cursor() -> Result<(), Error> {
    use hyper_filesystem::protocol::{DIRECTORY_MIN_BYTES, DirectoryBatch, DirectoryCursor};
    let mut volume = Volume::mount(Memory(large_directory_fixture(64)))?;
    let mut bytes = [0; DIRECTORY_MIN_BYTES];
    let length = volume.read_directory("", DirectoryCursor::START, &mut bytes)?;
    let batch = DirectoryBatch::decode(&bytes[..length]).ok_or(Error::Corrupt)?;
    assert!(!batch.eof());
    let cursor = batch.records().last().ok_or(Error::Corrupt)?.next_cursor();
    let length = volume.read_directory("", cursor, &mut bytes)?;
    let expected = bytes[..length].to_vec();
    let mut replacement = Volume::mount(volume.into_device())?;
    let length = replacement.read_directory("", cursor, &mut bytes)?;
    assert_eq!(&bytes[..length], expected);
    Ok(())
}
