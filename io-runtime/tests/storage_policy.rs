// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn partition_ownership_matches_broker_admission() -> Result<(), String> {
    let policy = parse(br#"{"native":[{"client":8,"source":"PARTLABEL=hyper-data","read_only":false}],"guests":[{"client":1,"source":"PARTLABEL=alpine","read_only":false}]}"#)?;
    let clients = crate::clients::parse(
        "hyper.clients.v3\n0 - - -\n1 alpine - -\n2 - default 02:48:59:00:00:02\n",
    )?;
    policy.validate_broker(&clients)?;
    assert!(policy.validate_broker(&clients[1..]).is_err());
    let native_exposed =
        crate::clients::parse("hyper.clients.v3\n0 - - -\n1 alpine - -\n8 data - -\n")?;
    assert!(policy.validate_broker(&native_exposed).is_err());
    assert!(Policy::default().validate_broker(&clients).is_err());
    Ok(())
}

#[test]
fn malformed_policy_cannot_authorize_management_or_duplicate_clients() {
    for text in [
        r#"{"native":[{"client":0,"source":"PARTLABEL=data","read_only":false}],"guests":[]}"#,
        r#"{"native":[{"client":9,"source":"PARTLABEL=data","read_only":false}],"guests":[]}"#,
        r#"{"native":[{"client":8,"source":"data","read_only":false}],"guests":[]}"#,
        r#"{"native":[{"client":8,"source":"PARTLABEL=data","read_only":false}],"guests":[{"client":8,"source":"PARTLABEL=guest","read_only":false}]}"#,
        r#"{"native":[{"client":8,"source":"PARTLABEL=data","read_only":false}],"guests":[{"client":1,"source":"PARTLABEL=data","read_only":false}]}"#,
        r#"{"native":[],"guests":[],"raw_disk":"sda"}"#,
    ] {
        assert!(parse(text.as_bytes()).is_err(), "{text}");
    }
    assert!(read(std::io::repeat(b' ')).is_err());
    assert!(parse(br#"{"native":[],"guests":[]}"#).is_ok());
}

#[test]
fn only_readonly_guest_sharing_can_repeat_a_selector() {
    assert!(parse(br#"{"native":[{"client":8,"source":"PARTLABEL=data","read_only":true}],"guests":[{"client":1,"source":"PARTLABEL=data","read_only":true},{"client":2,"source":"PARTLABEL=data","read_only":true}]}"#).is_ok());
    assert!(parse(br#"{"native":[{"client":8,"source":"PARTLABEL=data","read_only":true},{"client":7,"source":"PARTLABEL=data","read_only":true}],"guests":[]}"#).is_err());
}
