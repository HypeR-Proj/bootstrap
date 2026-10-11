// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::parent;
use hyper_os::{Error, Result};
use hyper_vm_image::guest_fdt::io::{FirmwareNode, FirmwareProperty};

pub(super) struct Node {
    pub path: String,
    pub properties: Vec<(String, Vec<u8>)>,
}
impl Node {
    pub fn set(&mut self, name: &str, value: &[u8]) {
        self.properties.retain(|(key, _)| key != name);
        self.properties.push((name.into(), value.into()));
    }
    pub fn set_cells(&mut self, name: &str, values: &[u32]) {
        self.set(
            name,
            &values
                .iter()
                .flat_map(|value| value.to_be_bytes())
                .collect::<Vec<_>>(),
        );
    }
}

pub struct Projection {
    pub(super) nodes: Vec<Node>,
}
impl Projection {
    pub(super) fn new(mut nodes: Vec<Node>) -> Result<Self> {
        nodes.sort_by(|left, right| left.path.cmp(&right.path));
        if nodes.len() > 256
            || nodes.iter().enumerate().any(|(index, node)| {
                node.path.split('/').count() > 8
                    || node.properties.len() > 32
                    || nodes[..index].iter().any(|other| other.path == node.path)
                    || (!parent(&node.path).is_empty()
                        && !nodes.iter().any(|other| other.path == parent(&node.path)))
            })
        {
            return Err(Error::InvalidResponse);
        }
        let bytes = nodes
            .iter()
            .try_fold(0usize, |sum, node| {
                node.properties
                    .iter()
                    .try_fold(sum.checked_add(node.path.len())?, |sum, (name, value)| {
                        sum.checked_add(name.len())?.checked_add(value.len())
                    })
            })
            .ok_or(Error::InvalidResponse)?;
        if bytes > 65536
            || nodes
                .iter()
                .map(|node| node.properties.len())
                .sum::<usize>()
                > 2048
        {
            return Err(Error::InvalidResponse);
        }
        Ok(Self { nodes })
    }

    /// Borrow one immutable tree without self-referential storage or leaked
    /// allocations. Each level borrows the already completed deeper level.
    pub fn with_nodes<R>(&self, use_nodes: impl FnOnce(&[FirmwareNode<'_>]) -> R) -> R {
        let properties: Vec<Vec<_>> = self
            .nodes
            .iter()
            .map(|node| {
                node.properties
                    .iter()
                    .map(|(name, value)| FirmwareProperty { name, value })
                    .collect()
            })
            .collect();
        let level7 = self.level(7, &[], &properties);
        let level6 = self.level(6, &level7, &properties);
        let level5 = self.level(5, &level6, &properties);
        let level4 = self.level(4, &level5, &properties);
        let level3 = self.level(3, &level4, &properties);
        let level2 = self.level(2, &level3, &properties);
        let level1 = self.level(1, &level2, &properties);
        let roots = self.level(0, &level1, &properties);
        use_nodes(&roots)
    }

    fn level<'a>(
        &'a self,
        depth: usize,
        children: &'a [FirmwareNode<'a>],
        properties: &'a [Vec<FirmwareProperty<'a>>],
    ) -> Vec<FirmwareNode<'a>> {
        let ordered = |level| {
            let mut nodes: Vec<_> = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| node.path.matches('/').count() == level)
                .collect();
            nodes.sort_by(|(_, left), (_, right)| {
                (parent(&left.path), left.path.as_str())
                    .cmp(&(parent(&right.path), right.path.as_str()))
            });
            nodes
        };
        let next = ordered(depth + 1);
        ordered(depth)
            .into_iter()
            .map(|(index, node)| {
                let start =
                    next.partition_point(|(_, child)| parent(&child.path) < node.path.as_str());
                let count = next[start..]
                    .iter()
                    .take_while(|(_, child)| parent(&child.path) == node.path)
                    .count();
                FirmwareNode {
                    name: node.path.rsplit('/').next().unwrap_or(&node.path),
                    properties: &properties[index],
                    children: &children[start..start + count],
                }
            })
            .collect()
    }
}
