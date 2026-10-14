// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::contracts::filesystem::Guid;

fn disk(id: u64) -> Disk {
    Disk {
        id,
        bytes: 16 * 1024 * 1024,
        sector_bytes: 512,
        physical_sector_bytes: 4096,
        read_only: false,
        next_cursor: id as u32,
    }
}

fn partition(id: u8, label: &str, first_lba: u64) -> gpt::Partition {
    gpt::Partition {
        id: Guid([id; 16]),
        kind: Guid([1; 16]),
        label: label.into(),
        first_lba,
        sectors: 1024,
        attributes: 0,
    }
}

fn volume(client: u32, source: &str, read_only: bool) -> Volume {
    Volume {
        client,
        source: source.into(),
        read_only,
    }
}

#[test]
fn ambiguous_labels_require_an_unambiguous_uuid() -> Result<(), String> {
    let mut catalog = Catalog::default();
    let expected = partition(0xab, "data", 2048);
    catalog.extend(disk(1), vec![expected.clone()])?;
    catalog.extend(disk(2), vec![partition(2, "data", 2048)])?;
    let mut policy = Policy {
        native: vec![volume(8, "PARTLABEL=data", false)],
        guests: vec![],
    };
    assert!(catalog.resolve(&policy).is_err());
    policy.native[0].source = format!("PARTUUID={}", expected.id).to_uppercase();
    let Resolution::Ready(selected) = catalog.resolve(&policy)? else {
        return Err("UUID did not resolve".into());
    };
    assert_eq!(selected[0].disk.id, 1);
    assert_eq!(selected[0].partition, expected);
    Ok(())
}

#[test]
fn aliases_cannot_bypass_writable_overlap_checks() -> Result<(), String> {
    let mut catalog = Catalog::default();
    let part = partition(2, "data", 2048);
    let uuid = format!("PARTUUID={}", part.id);
    catalog.extend(disk(1), vec![part])?;
    for (native_read_only, guest_read_only) in [(false, true), (true, false), (false, false)] {
        let policy = Policy {
            native: vec![volume(8, "PARTLABEL=data", native_read_only)],
            guests: vec![volume(1, &uuid, guest_read_only)],
        };
        assert!(catalog.resolve(&policy).is_err());
    }
    let policy = Policy {
        native: vec![volume(8, "PARTLABEL=data", true)],
        guests: vec![volume(1, &uuid, true), volume(2, "PARTLABEL=data", true)],
    };
    assert!(matches!(catalog.resolve(&policy)?, Resolution::Ready(parts) if parts.len() == 3));
    Ok(())
}

#[test]
fn native_filesystem_identities_must_be_unique_even_when_read_only() -> Result<(), String> {
    let mut catalog = Catalog::default();
    let part = partition(2, "data", 2048);
    let uuid = format!("PARTUUID={}", part.id);
    catalog.extend(disk(1), vec![part.clone()])?;
    let policy = Policy {
        native: vec![volume(8, "PARTLABEL=data", true), volume(7, &uuid, true)],
        guests: vec![],
    };
    assert!(catalog.resolve(&policy).is_err());

    // A cloned GPT on another disk must not introduce the same filesystem
    // identity through a different label either.
    let mut clone = part;
    clone.label = "copy".into();
    catalog.extend(disk(2), vec![clone])?;
    let policy = Policy {
        native: vec![
            volume(8, "PARTLABEL=data", true),
            volume(7, "PARTLABEL=copy", true),
        ],
        guests: vec![],
    };
    assert!(catalog.resolve(&policy).is_err());
    Ok(())
}

#[test]
fn assignments_preserve_policy_order_and_adjacent_ranges() -> Result<(), String> {
    let mut catalog = Catalog::default();
    catalog.extend(
        disk(1),
        vec![partition(1, "guest", 3072), partition(2, "data", 2048)],
    )?;
    let policy = Policy {
        native: vec![volume(8, "PARTLABEL=data", false)],
        guests: vec![volume(1, "PARTLABEL=guest", false)],
    };
    let Resolution::Ready(selected) = catalog.resolve(&policy)? else {
        return Err("adjacent ranges did not resolve".into());
    };
    assert_eq!(selected[0].partition.label, "data");
    assert_eq!(selected[1].partition.label, "guest");
    Ok(())
}

#[test]
fn missing_media_is_retryable_but_ambiguity_is_not() -> Result<(), String> {
    let mut catalog = Catalog::default();
    let mut policy = Policy {
        native: vec![volume(8, "PARTLABEL=data", false)],
        guests: vec![volume(1, "PARTLABEL=guest", false)],
    };
    let Resolution::Missing(missing) = catalog.resolve(&policy)? else {
        return Err("empty catalog should report missing media".into());
    };
    assert_eq!(missing, ["PARTLABEL=data", "PARTLABEL=guest"]);
    catalog.extend(disk(1), vec![partition(1, "guest", 2048)])?;
    assert!(
        matches!(catalog.resolve(&policy)?, Resolution::Missing(parts) if parts == ["PARTLABEL=data"])
    );
    catalog.extend(disk(2), vec![partition(2, "guest", 2048)])?;
    assert!(catalog.resolve(&policy).is_err());
    policy.guests.clear();
    catalog.extend(disk(3), vec![partition(3, "data", 2048)])?;
    assert!(matches!(catalog.resolve(&policy)?, Resolution::Ready(_)));
    Ok(())
}

#[test]
fn readonly_media_and_native_sector_geometry_are_enforced() -> Result<(), String> {
    let mut catalog = Catalog::default();
    let mut media = disk(1);
    media.read_only = true;
    media.sector_bytes = 4096;
    catalog.extend(media, vec![partition(1, "data", 128)])?;
    let mut policy = Policy {
        native: vec![],
        guests: vec![volume(1, "PARTLABEL=data", false)],
    };
    assert!(catalog.resolve(&policy).is_err());
    policy.guests[0].read_only = true;
    assert!(matches!(catalog.resolve(&policy)?, Resolution::Ready(_)));
    policy.native = std::mem::take(&mut policy.guests);
    assert!(catalog.resolve(&policy).is_err());
    Ok(())
}

#[test]
fn catalog_limit_is_checked_before_mutating_entries() -> Result<(), String> {
    let mut catalog = Catalog::default();
    let part = partition(1, "data", 2048);
    catalog.extend(disk(1), vec![part.clone(); MAX_PARTITIONS])?;
    assert!(catalog.extend(disk(2), vec![part]).is_err());
    assert_eq!(catalog.partitions.len(), MAX_PARTITIONS);
    assert!(catalog.partitions.iter().all(|entry| entry.disk.id == 1));
    Ok(())
}
