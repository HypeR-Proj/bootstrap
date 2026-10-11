// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Normalize board bindings understood by the packaged upstream Linux drivers.
//! This translates description data only; Linux performs the actual reset.

use super::{cells, descendant, parent, tree::Node};
use hyper_os::{Error, Result};

pub(super) fn phy_reset(nodes: &mut Vec<Node>) -> Result<()> {
    let macs: Vec<_> = nodes
        .iter()
        .filter(|node| {
            enabled(node)
                && !nodes
                    .iter()
                    .any(|ancestor| descendant(&node.path, &ancestor.path) && !enabled(ancestor))
                && property(node, "compatible").is_some_and(|value| {
                    value
                        .split(|byte| *byte == 0)
                        .any(|value| value == b"raspberrypi,rp1-gem")
                })
        })
        .map(|node| node.path.clone())
        .collect();
    for mac_path in macs {
        let mac_index = nodes
            .iter()
            .position(|node| node.path == mac_path)
            .ok_or(Error::InvalidResponse)?;
        let mac = &nodes[mac_index];
        let Some(gpios) = property(mac, "phy-reset-gpios") else {
            continue;
        };
        let gpios = gpios.to_vec();
        let duration = cells(property(mac, "phy-reset-duration").ok_or(Error::InvalidResponse)?)?;
        let [duration] = duration.as_slice() else {
            return Err(Error::InvalidResponse);
        };
        let micros = duration.checked_mul(1000).ok_or(Error::InvalidResponse)?;
        let mdio_path = format!("{mac_path}/mdio");
        let mdio_index = match nodes.iter().position(|node| node.path == mdio_path) {
            Some(index) => index,
            None => {
                nodes.push(Node {
                    path: mdio_path.clone(),
                    properties: Vec::new(),
                });
                nodes.len() - 1
            }
        };
        let mdio = &mut nodes[mdio_index];
        set_consistent(mdio, "reset-gpios", &gpios)?;
        set_consistent(mdio, "reset-delay-us", &micros.to_be_bytes())?;
        set_consistent(mdio, "#address-cells", &1u32.to_be_bytes())?;
        set_consistent(mdio, "#size-cells", &0u32.to_be_bytes())?;
        // Direct PHY children are the old MAC-level MDIO binding. Preserve
        // every such PHY and its descendants, including their original IDs.
        let phys: Vec<_> = nodes
            .iter()
            .filter(|node| {
                parent(&node.path) == mac_path
                    && node.path != mdio_path
                    && property(node, "reg").is_some_and(|bytes| {
                        bytes.len() == 4
                            && u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) < 32
                    })
            })
            .map(|node| node.path.clone())
            .collect();
        for phy in phys {
            for node in nodes
                .iter_mut()
                .filter(|node| node.path == phy || descendant(&node.path, &phy))
            {
                let suffix = node
                    .path
                    .strip_prefix(&mac_path)
                    .ok_or(Error::InvalidResponse)?;
                node.path = format!("{mdio_path}{suffix}");
            }
        }
        nodes[mac_index]
            .properties
            .retain(|(name, _)| !matches!(name.as_str(), "phy-reset-gpios" | "phy-reset-duration"));
    }
    Ok(())
}

fn property<'a>(node: &'a Node, name: &str) -> Option<&'a [u8]> {
    node.properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_slice())
}
fn set_consistent(node: &mut Node, name: &str, value: &[u8]) -> Result<()> {
    if property(node, name).is_some_and(|existing| existing != value) {
        return Err(Error::InvalidResponse);
    }
    node.set(name, value);
    Ok(())
}

fn enabled(node: &Node) -> bool {
    matches!(property(node, "status"), None | Some(b"ok\0" | b"okay\0"))
}
