// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use crate::contracts::io::{
    CONNECT_BYTES, Connection, DISK, NETWORK, decode_connect, encode_connect, parse_mac, valid_name,
};

const MAC: [u8; 6] = [2, 0x48, 0x59, 0, 0, 1];

#[test]
fn each_device_set_has_one_canonical_connection() -> Result<(), &'static str> {
    for (devices, volume, network) in [
        (DISK, Some("alpine"), None),
        (NETWORK, None, Some("default")),
        (DISK | NETWORK, Some("alpine"), Some("default")),
    ] {
        let connection = Connection {
            client: 7,
            volume,
            network,
            mac: if network.is_some() { MAC } else { [0; 6] },
        };
        let bytes = connection.encode().ok_or("valid connection rejected")?;
        assert_eq!(bytes.len(), CONNECT_BYTES);
        assert_eq!(&bytes[..8], b"HIOCONN2");
        assert_eq!(&bytes[8..12], &7u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &devices.to_le_bytes());
        assert_eq!(&bytes[80..86], &connection.mac);
        assert_eq!(&bytes[86..], &[0; 10]);
        assert_eq!(decode_connect(&bytes), Some(connection));
        assert_eq!(decode_connect(&bytes[..CONNECT_BYTES - 1]), None);
        for offset in [0, 7, 15, 47, 79, 86, 95] {
            let mut invalid = bytes;
            invalid[offset] ^= 0xff;
            assert_eq!(decode_connect(&invalid), None, "offset {offset}");
        }
        let mut invalid = bytes;
        invalid[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode_connect(&invalid), None);
    }
    Ok(())
}

#[test]
fn connection_rejects_implicit_native_empty_and_hidden_authority() -> Result<(), &'static str> {
    let valid = Connection {
        client: 1,
        volume: Some("alpine"),
        network: None,
        mac: [0; 6],
    };
    for client in [0, 128, u32::MAX] {
        assert!(Connection { client, ..valid }.encode().is_none());
    }
    assert!(
        Connection {
            volume: None,
            ..valid
        }
        .encode()
        .is_none()
    );
    assert!(Connection { mac: MAC, ..valid }.encode().is_none());
    for network in ["", "Unknown", "../default", "default_net", "a\0b"] {
        assert!(
            Connection {
                network: Some(network),
                mac: MAC,
                ..valid
            }
            .encode()
            .is_none()
        );
    }
    let bytes = valid.encode().ok_or("disk connection rejected")?;
    assert_eq!(encode_connect(1, "alpine"), Some(bytes));
    for offset in [48, 80] {
        let mut invalid = bytes;
        invalid[offset] = 1;
        assert!(decode_connect(&invalid).is_none());
    }
    let network = Connection {
        volume: None,
        network: Some("default"),
        mac: MAC,
        ..valid
    };
    let mut hidden_volume = network.encode().ok_or("network connection rejected")?;
    hidden_volume[16] = b'x';
    assert_eq!(decode_connect(&hidden_volume), None);
    Ok(())
}

#[test]
fn disk_tokens_keep_the_existing_full_width_contract() -> Result<(), &'static str> {
    let volume = "Abcdefghijklmnopqrstuvwxyz12-_34";
    assert_eq!(volume.len(), 32);
    let bytes = encode_connect(127, volume).ok_or("existing disk token rejected")?;
    assert_eq!(
        decode_connect(&bytes).and_then(|value| value.volume),
        Some(volume)
    );
    assert!(encode_connect(1, "Abcdefghijklmnopqrstuvwxyz12-_345").is_none());
    assert!(encode_connect(1, "a\0b").is_none());
    Ok(())
}

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
