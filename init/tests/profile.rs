// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn profile(kind: &str, handoff: bool) -> serde_json::Value {
    let mut value = serde_json::json!({
        "version": 1, "services": "/etc/hyper/services.json",
        "io": {"kind": kind}, "requires": ["/svc/console-raw"],
    });
    if kind != "none" {
        value["io"]["service"] = "disk".into();
        if handoff {
            value["io"]["successor"] = "/native-disk".into();
        }
    }
    if handoff {
        value["handoff"] = serde_json::json!({
            "system-root": "/system", "put-old": "/boot",
            "system-init": "/svc/component-manager",
        });
    }
    value
}

fn manifest(vm: bool) -> serde_json::Value {
    let mut value = serde_json::json!({
        "format": "hyper.service-manifest", "services": [{
            "name": "disk", "image": "/svc/custom-disk", "critical": true,
            "restart": "never", "after": [], "capabilities": [{
                "source": "bootstrap.io-ready-channel", "purpose": "io.ready",
                "operation": "move", "rights": ["wait", "write"],
            }],
        }],
    });
    if vm && let Some(capabilities) = value["services"][0]["capabilities"].as_array_mut() {
        capabilities.push(serde_json::json!({
            "source": "bootstrap.vm-creation-authority", "purpose": "vm.creation-authority",
            "operation": "duplicate", "rights": ["derive", "create-virtual-machine"],
        }));
    }
    value
}

#[test]
fn selector_is_defaulted_bounded_and_unambiguous() {
    assert_eq!(select("").as_deref(), Ok("default"));
    assert_eq!(select("earlycon=x nokaslr").as_deref(), Ok("default"));
    assert_eq!(
        select("hyper.boot-profile=native-disk_2 loglevel=7").as_deref(),
        Ok("native-disk_2")
    );
    for command in [
        "hyper.boot-profile",
        "hyper.boot-profile=",
        "hyper.boot-profile=../default",
        "hyper.boot-profile=/default",
        "hyper.boot-profile=a hyper.boot-profile=a",
        "hyper.boot-profile=a hyper.boot-profile=b",
        "hyper.boot-profile=a.json",
    ] {
        assert!(select(command).is_err(), "{command}");
    }
    assert!(select(&format!("hyper.boot-profile={}", "a".repeat(64))).is_err());
}

#[test]
fn storage_owner_cannot_also_claim_console_handoff() -> Result<(), String> {
    let profile = Profile::parse(&profile("service", true).to_string())?;
    let mut document = manifest(false);
    document["services"][0]["image"] = "/svc/console-raw".into();
    document["services"][0]["capabilities"]
        .as_array_mut()
        .ok_or("capability array")?
        .push(serde_json::json!({
            "source": "bootstrap.console", "purpose": "console.system",
            "operation": "duplicate", "rights": ["wait", "read", "write"],
        }));
    let source = document.to_string();
    let manifest = manifest::parse(&source).map_err(|error| format!("{error:?}"))?;
    assert!(profile.validate(&manifest).is_err());
    Ok(())
}

#[test]
fn profile_rejects_unknown_shape_and_incomplete_migration() {
    for kind in ["none", "service", "io-vm"] {
        for handoff in [false, true] {
            assert!(Profile::parse(&profile(kind, handoff).to_string()).is_ok());
        }
    }
    let valid = profile("service", true);
    for (field, value) in [
        ("version", 2.into()),
        ("unknown", true.into()),
        ("services", "/etc/../services".into()),
        ("requires", serde_json::json!(["/a", "/a"])),
        ("requires", serde_json::json!(["relative"])),
        ("io", serde_json::json!({"kind":"none", "service":"disk"})),
        (
            "io",
            serde_json::json!({"kind":"service", "service":"disk"}),
        ),
    ] {
        let mut bad = valid.clone();
        bad[field] = value;
        assert!(Profile::parse(&bad.to_string()).is_err(), "{bad}");
    }
    for root in ["/", "relative", "/x/../system", "//system", "/system/"] {
        let mut bad = valid.clone();
        bad["handoff"]["system-root"] = root.into();
        assert!(Profile::parse(&bad.to_string()).is_err());
    }
    assert!(Profile::parse(&" ".repeat(MAX_BYTES + 1)).is_err());
}

#[test]
fn generic_provider_needs_no_io_runtime_image_or_vm_authority() -> Result<(), String> {
    let text = manifest(false).to_string();
    let manifest = manifest::parse(&text).map_err(|error| format!("{error:?}"))?;
    let native = Profile::parse(&profile("service", true).to_string())?;
    assert_eq!(native.storage_service(&manifest)?, Some(0));
    let plan = native.validate(&manifest)?;
    assert_eq!(
        crate::bootstrap_policy::io_ready_service(&plan),
        Ok(Some(0))
    );
    assert!(
        Profile::parse(&profile("io-vm", true).to_string())?
            .validate(&manifest)
            .is_err()
    );
    assert!(
        Profile::parse(&profile("none", false).to_string())?
            .validate(&manifest)
            .is_err()
    );
    Ok(())
}

#[test]
fn vm_authority_is_tied_to_selected_io_vm_provider() -> Result<(), String> {
    let text = manifest(true).to_string();
    let manifest = manifest::parse(&text).map_err(|error| format!("{error:?}"))?;
    let vm = Profile::parse(&profile("io-vm", true).to_string())?;
    assert!(vm.validate(&manifest).is_ok());
    let native = Profile::parse(&profile("service", true).to_string())?;
    assert!(native.validate(&manifest).is_err());
    let mut unknown = profile("io-vm", true);
    unknown["io"]["service"] = "missing".into();
    assert!(
        Profile::parse(&unknown.to_string())?
            .validate(&manifest)
            .is_err()
    );
    Ok(())
}

#[test]
fn provider_pair_and_unselected_vm_grants_are_rejected() -> Result<(), String> {
    let mut source = manifest(false);
    source["services"][0]["capabilities"] = serde_json::json!([{
        "source": "bootstrap.filesystem-provider", "purpose": "filesystem.provider",
        "operation": "move", "rights": ["wait", "write"],
    }]);
    let text = source.to_string();
    let parsed = manifest::parse(&text).map_err(|error| format!("{error:?}"))?;
    assert!(
        Profile::parse(&profile("service", true).to_string())?
            .validate(&parsed)
            .is_err()
    );
    source = manifest(true);
    let mut second = source["services"][0].clone();
    second["name"] = "second".into();
    second["capabilities"] = serde_json::json!([second["capabilities"][1].clone()]);
    let services = source["services"]
        .as_array_mut()
        .ok_or("missing services")?;
    services.push(second);
    let text = source.to_string();
    let parsed = manifest::parse(&text).map_err(|error| format!("{error:?}"))?;
    assert!(
        Profile::parse(&profile("io-vm", true).to_string())?
            .validate(&parsed)
            .is_err()
    );
    Ok(())
}

#[test]
fn guest_only_backend_does_not_gate_an_independently_prepared_root() -> Result<(), String> {
    let mut source = manifest(true);
    let grants = source["services"][0]["capabilities"]
        .as_array_mut()
        .ok_or("missing grants")?;
    grants.remove(0);
    let text = source.to_string();
    let parsed = manifest::parse(&text).map_err(|error| format!("{error:?}"))?;
    assert!(
        Profile::parse(&profile("io-vm", true).to_string())?
            .validate(&parsed)
            .is_ok()
    );
    Ok(())
}
