// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use crate::contracts::io::{parse_mac, valid_name, valid_volume};

#[test]
fn disk_tokens_keep_the_full_width_contract() {
    assert!(valid_volume("Abcdefghijklmnopqrstuvwxyz12-_34"));
    assert!(!valid_volume("Abcdefghijklmnopqrstuvwxyz12-_345"));
    assert!(!valid_volume(""));
    assert!(!valid_volume("a\0b"));
    assert!(!valid_volume("../disk"));
    assert!(!valid_volume("disk name"));
}

const MAC: [u8; 6] = [2, 0x48, 0x59, 0, 0, 1];

#[test]
fn network_identity_is_canonical_and_locally_administered() {
    assert_eq!(parse_mac("02:48:59:00:00:01"), Some(MAC));
    assert!(valid_name("default-net1"));
    assert!(valid_name("abcdefghijklmnopqrstuvwxyz12345"));
    assert!(!valid_name("abcdefghijklmnopqrstuvwxyz123456"));
    for value in [
        "",
        "00:48:59:00:00:01",
        "03:48:59:00:00:01",
        "ff:ff:ff:ff:ff:ff",
        "02:48:59:00:00:AA",
        "02:48:59:00:00",
        "02:48:59:00:00:01\n",
        "02-48-59-00-00-01",
    ] {
        assert_eq!(parse_mac(value), None);
    }
}
