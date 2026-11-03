// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Board and handoff identity validation; no formal client admission codec.

pub const DISK: u32 = 1;
pub const NETWORK: u32 = 2;

/// A deployment token shared by board policy and the Linux bootstrap format.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 31
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Existing disk identities permit a full 32-byte token without a terminator.
pub fn valid_volume(volume: &str) -> bool {
    !volume.is_empty()
        && volume.len() <= 32
        && volume
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

/// Accept only canonical, locally administered unicast Ethernet addresses.
pub fn parse_mac(value: &str) -> Option<[u8; 6]> {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }
    let bytes = value.as_bytes();
    if bytes.len() != 17 {
        return None;
    }
    let mut mac = [0; 6];
    for (index, octet) in mac.iter_mut().enumerate() {
        let offset = index * 3;
        *octet = hex(bytes[offset])? << 4 | hex(bytes[offset + 1])?;
        if index != 5 && bytes[offset + 2] != b':' {
            return None;
        }
    }
    (mac[0] & 3 == 2).then_some(mac)
}
