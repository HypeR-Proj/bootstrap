// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::BootstrapPolicy;
use crate::manifest::{self, AuthorityPolicy, ValidationErrorKind};
use hyper_os::handle::Rights;

const POLICY_SERVICE: &str = r#"{
  "format": "hyper.service-manifest",
  "services": [{
    "name": "policy", "image": "/svc/power-policy", "critical": true,
    "restart": "never", "after": [], "capabilities": [
      {"source":"bootstrap.service-output-channel", "purpose":"stdio.output",
       "operation":"duplicate", "rights":["wait","write"]},
      {"source":"bootstrap.root-directory", "purpose":"process.root-directory",
       "operation":"duplicate", "rights":["read"]},
      {"source":"bootstrap.cpu-inspector", "purpose":"process.cpu-inspector",
       "operation":"duplicate", "rights":["inspect"]}
    ]
  }]
}"#;

#[test]
fn ordinary_policy_service_gets_only_explicit_attenuated_authority() -> Result<(), String> {
    let manifest = manifest::parse(POLICY_SERVICE).map_err(|e| format!("{e:?}"))?;
    let plan = manifest::validate(&manifest, &BootstrapPolicy).map_err(|e| format!("{e:?}"))?;
    for (index, rights) in [
        Rights::WAIT.union(Rights::WRITE),
        Rights::READ,
        Rights::INSPECT,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            plan.capability_grant(0, index).map(|grant| grant.rights()),
            Some(rights.bits())
        );
    }
    assert!(plan.capability_grant(0, 3).is_none());
    Ok(())
}

#[test]
fn mixed_deployment_sources_are_not_boot_graphs() -> Result<(), String> {
    for text in [
        include_str!("config/services.json"),
        include_str!("../config/services-with-vms.json"),
        include_str!("../config/services-console-only.json"),
    ] {
        assert!(manifest::parse(text).is_err());
    }
    Ok(())
}

#[test]
fn boot_graph_requires_static_services_even_without_private_roles() -> Result<(), String> {
    let static_service = POLICY_SERVICE.replace(
        "\"critical\": true",
        "\"loader\": \"bootstrap\", \"critical\": true",
    );
    let manifest = manifest::parse(&static_service).map_err(|e| format!("{e:?}"))?;
    manifest::validate(&manifest, &BootstrapPolicy).map_err(|e| format!("{e:?}"))?;
    for text in [
        static_service.replace("\"loader\": \"bootstrap\"", "\"loader\": \"dynamic\""),
        static_service.replace(
            "\"loader\": \"bootstrap\"",
            "\"loader\": \"bootstrap\", \"phase\": \"system\"",
        ),
    ] {
        assert!(manifest::parse(&text).is_err());
    }
    Ok(())
}

#[test]
fn formal_control_roles_are_absent_from_bootstrap_policy() {
    for source in [
        "bootstrap.vm-runtime-image",
        "bootstrap.vm-provisioning-channel",
        "bootstrap.vm-manager-connection-channel",
        "bootstrap.vm-client-connection-channel",
    ] {
        assert!(BootstrapPolicy.authority(source).is_none(), "{source}");
    }
    for (image, purpose) in [
        ("/svc/session-manager", "session.console-controller"),
        ("/svc/session-manager", "session.log-connector"),
        ("/svc/vm-manager", "vm.provisioning"),
        ("/svc/vm-manager", "vm.runtime-image"),
        ("/svc/vm-manager", "vm.creation-authority"),
        ("/bin/sh", "vm.manager-connection"),
    ] {
        assert!(BootstrapPolicy.startup_purpose(image, purpose).is_none());
    }
    assert!(
        BootstrapPolicy
            .startup_purpose("/svc/io-runtime", "vm.creation-authority")
            .is_some()
    );
}

#[test]
fn common_contracts_do_not_expose_private_service_roles_or_weaken_required_rights()
-> Result<(), String> {
    for purpose in [
        "console.system",
        "io.device-authority",
        "filesystem.attach",
        "filesystem.provider",
        "vm.provisioning",
        "vm.creation-authority",
        "session.client-input",
    ] {
        assert!(
            BootstrapPolicy
                .startup_purpose("/svc/power-policy", purpose)
                .is_none()
        );
    }
    for (old, new, expected) in [
        (
            "[\"wait\",\"write\"]",
            "[\"wait\"]",
            ValidationErrorKind::MissingRequiredRights,
        ),
        (
            "[\"wait\",\"write\"]",
            "[\"wait\",\"write\",\"read\"]",
            ValidationErrorKind::RightsEscalation,
        ),
    ] {
        let text = POLICY_SERVICE.replace(old, new);
        let manifest = manifest::parse(&text).map_err(|e| format!("{e:?}"))?;
        assert_eq!(
            manifest::validate(&manifest, &BootstrapPolicy)
                .err()
                .map(|e| e.kind()),
            Some(expected)
        );
    }
    Ok(())
}

#[test]
fn filesystem_requires_both_distinct_explicit_owners() -> Result<(), String> {
    const MANIFEST: &str = r#"{"format":"hyper.service-manifest","services":[
      {"name":"filesystem","image":"/svc/fs-backend","critical":false,"restart":"never","after":[],"capabilities":[
        {"source":"bootstrap.filesystem-attach","purpose":"filesystem.attach","operation":"move","rights":["wait","read"]}]},
      {"name":"storage","image":"/svc/io-runtime","critical":false,"restart":"never","after":[],"capabilities":[
        {"source":"bootstrap.filesystem-provider","purpose":"filesystem.provider","operation":"move","rights":["wait","write"]}]}
    ]}"#;
    let manifest = manifest::parse(MANIFEST).map_err(|e| format!("{e:?}"))?;
    let plan = manifest::validate(&manifest, &BootstrapPolicy).map_err(|e| format!("{e:?}"))?;
    assert_eq!(super::filesystem_enabled(&plan), Ok(true));
    let missing_peer = MANIFEST.replace(
        r#"{"source":"bootstrap.filesystem-provider","purpose":"filesystem.provider","operation":"move","rights":["wait","write"]}"#,
        "",
    );
    let manifest = manifest::parse(&missing_peer).map_err(|e| format!("{e:?}"))?;
    let plan = manifest::validate(&manifest, &BootstrapPolicy).map_err(|e| format!("{e:?}"))?;
    assert_eq!(
        super::filesystem_enabled(&plan),
        Err(super::IoReadyPlanError::InvalidGrant)
    );
    Ok(())
}

const IO_READY_SERVICE: &str = r#"{
  "format": "hyper.service-manifest",
  "services": [{
    "name": "storage", "image": "/svc/io-runtime", "critical": true,
    "restart": "never", "after": [], "capabilities": [
      {"source":"bootstrap.io-ready-channel", "purpose":"io.ready",
       "operation":"move", "rights":["wait","write"]}
    ]
  }]
}"#;

#[test]
fn storage_readiness_requires_the_dedicated_moved_authority() -> Result<(), String> {
    let manifest = manifest::parse(IO_READY_SERVICE).map_err(|e| format!("{e:?}"))?;
    let plan = manifest::validate(&manifest, &BootstrapPolicy).map_err(|e| format!("{e:?}"))?;
    assert_eq!(super::io_ready_service(&plan), Ok(Some(0)));
    let text = IO_READY_SERVICE.replace("io.ready", "stdio.output");
    let manifest = manifest::parse(&text).map_err(|e| format!("{e:?}"))?;
    let plan = manifest::validate(&manifest, &BootstrapPolicy).map_err(|e| format!("{e:?}"))?;
    assert_eq!(
        super::io_ready_service(&plan),
        Err(super::IoReadyPlanError::InvalidGrant)
    );
    Ok(())
}

#[test]
fn ordinary_services_do_not_imply_storage_readiness() -> Result<(), String> {
    let manifest = manifest::parse(POLICY_SERVICE).map_err(|e| format!("{e:?}"))?;
    let plan = manifest::validate(&manifest, &BootstrapPolicy).map_err(|e| format!("{e:?}"))?;
    assert_eq!(super::io_ready_service(&plan), Ok(None));
    assert!(
        BootstrapPolicy
            .startup_purpose("/svc/power-policy", "io.ready")
            .is_none()
    );
    Ok(())
}

#[test]
fn virtual_console_owns_shell_launch_and_bootstrap_rejects_old_client_endpoints() {
    for source in [
        "bootstrap.shell-input-channel",
        "bootstrap.session-client-input-channel",
    ] {
        assert!(BootstrapPolicy.authority(source).is_none());
    }
}

#[test]
fn ordinary_services_cannot_receive_namespace_management_authority() {
    assert!(!super::root_directory_rights().contains(Rights::MANAGE_NAMESPACE));
}
