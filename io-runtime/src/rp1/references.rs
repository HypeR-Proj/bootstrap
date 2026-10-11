// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Standard firmware reference encodings, used only to close the description
//! graph. Values are preserved; no raw word is guessed to be a phandle.

use super::{cell, cells};
use crate::firmware::FirmwareNode;
use hyper_os::{Error, Result};

pub(super) fn referenced(nodes: &[FirmwareNode], node: &FirmwareNode) -> Result<Vec<usize>> {
    let mut result = Vec::new();
    for (name, bytes) in &node.properties {
        let specifier = match name.as_str() {
            "clocks" | "assigned-clocks" | "assigned-clock-parents" => Some("#clock-cells"),
            "resets" => Some("#reset-cells"),
            "dmas" => Some("#dma-cells"),
            "mboxes" => Some("#mbox-cells"),
            "phys" => Some("#phy-cells"),
            "iommus" => Some("#iommu-cells"),
            "power-domains" => Some("#power-domain-cells"),
            "interrupts-extended" => Some("#interrupt-cells"),
            "gpio" | "gpios" => Some("#gpio-cells"),
            _ if name.ends_with("-gpios") || name.ends_with("-gpio") => Some("#gpio-cells"),
            _ => None,
        };
        let handles = matches!(
            name.as_str(),
            "interrupt-parent"
                | "phy-handle"
                | "firmware"
                | "shmem"
                | "memory-region"
                | "nvmem-cells"
                | "remote-endpoint"
                | "operating-points-v2"
        ) || name.ends_with("-supply")
            || name.strip_prefix("pinctrl-").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            });
        if specifier.is_none() && !handles {
            if matches!(
                name.as_str(),
                "interrupt-map" | "iommu-map" | "msi-map" | "msi-parent"
            ) {
                return Err(Error::InvalidResponse);
            }
            continue;
        }
        let values = cells(bytes)?;
        let mut cursor = 0;
        while cursor < values.len() {
            let handle = values[cursor];
            cursor += 1;
            // Null entries are defined by clock assignment and GPIO bindings.
            if handle == 0 {
                continue;
            }
            let mut matching = nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| cell(node, "phandle").ok() == Some(handle));
            let (index, target) = matching.next().ok_or(Error::InvalidResponse)?;
            if matching.next().is_some() {
                return Err(Error::InvalidResponse);
            }
            let count = match specifier {
                Some(name) => cell(target, name)? as usize,
                None => 0,
            };
            if count > 16 || count > values.len() - cursor {
                return Err(Error::InvalidResponse);
            }
            cursor += count;
            if !result.contains(&index) {
                result.push(index);
            }
        }
    }
    Ok(result)
}
