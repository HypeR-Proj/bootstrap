// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Resolve on-disk identities before publishing any backend assignment.

use super::{Policy, Volume};
use crate::protocol::block::Disk;
use hyper_block::{Selector, gpt};

const MAX_PARTITIONS: usize = 4096;

#[derive(Clone, Debug)]
pub struct Selection {
    pub disk: Disk,
    pub partition: gpt::Partition,
}

#[derive(Default)]
pub struct Catalog {
    partitions: Vec<Selection>,
}

pub enum Resolution {
    Missing(Vec<String>),
    Ready(Vec<Selection>),
}

impl Catalog {
    pub fn extend(&mut self, disk: Disk, partitions: Vec<gpt::Partition>) -> Result<(), String> {
        if partitions.len() > MAX_PARTITIONS - self.partitions.len() {
            return Err("too many discovered partitions".into());
        }
        self.partitions.extend(
            partitions
                .into_iter()
                .map(|partition| Selection { disk, partition }),
        );
        Ok(())
    }

    fn select(&self, volume: &Volume, native: bool) -> Result<Option<Selection>, String> {
        let selector = Selector::parse(&volume.source).ok_or("invalid partition selector")?;
        let mut matches = self
            .partitions
            .iter()
            .filter(|entry| selector.matches(entry.partition.id, &entry.partition.label));
        let Some(result) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(format!("ambiguous partition selector: {}", volume.source));
        }
        if native && result.disk.sector_bytes != 512 {
            return Err("Native SCSI initiator currently requires 512-byte logical sectors".into());
        }
        if result.disk.read_only && !volume.read_only {
            return Err(format!(
                "{} requests writes on read-only media",
                volume.source
            ));
        }
        Ok(Some(result.clone()))
    }

    /// Ready entries follow Native-then-guest policy order. Missing media may
    /// be retried, while ambiguous identities or conflicting ownership fail
    /// before the caller sends even the first immutable `BLOCK_BIND` request.
    pub fn resolve(&self, policy: &Policy) -> Result<Resolution, String> {
        let volumes: Vec<_> = policy.native.iter().chain(&policy.guests).collect();
        let mut selected: Vec<Selection> = Vec::new();
        let mut missing = Vec::new();
        for (index, volume) in volumes.iter().enumerate() {
            match self.select(volume, index < policy.native.len())? {
                Some(selection) => selected.push(selection),
                None => missing.push(volume.source.clone()),
            }
        }
        if !missing.is_empty() {
            return Ok(Resolution::Missing(missing));
        }
        for (index, current) in selected.iter().enumerate() {
            for (old_index, earlier) in selected[..index].iter().enumerate() {
                if index < policy.native.len() && current.partition.id == earlier.partition.id {
                    return Err("duplicate Native partition identity".into());
                }
                if current.disk.id != earlier.disk.id {
                    continue;
                }
                if !volumes[index].read_only || !volumes[old_index].read_only {
                    let current_end = current
                        .partition
                        .first_lba
                        .checked_add(current.partition.sectors)
                        .ok_or("partition range overflow")?;
                    let earlier_end = earlier
                        .partition
                        .first_lba
                        .checked_add(earlier.partition.sectors)
                        .ok_or("partition range overflow")?;
                    if current.partition.first_lba < earlier_end
                        && earlier.partition.first_lba < current_end
                    {
                        return Err("overlapping writable partition assignments".into());
                    }
                }
            }
        }
        Ok(Resolution::Ready(selected))
    }
}

#[cfg(test)]
#[path = "../../tests/storage_catalog.rs"]
mod tests;
