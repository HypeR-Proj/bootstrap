// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

extern crate std;

use std::fmt::Write;
use std::string::String;

use super::{
    AuthorityDeclaration, AuthorityKey, AuthorityPolicy, MAX_MANIFEST_BYTES, MAX_SERVICES,
    ParseErrorKind, StartupPurposeDeclaration, ValidationErrorKind, parse, validate,
};

// This parser test policy also exercises arbitrary formal roles. They are
// deliberately absent from the production bootstrap capability vocabulary.
const MOCK_VM_PROVISIONING_PURPOSE: u32 = 0x8005_0004;

const VALID: &str = r#"
{
  "format": "hyper.service-manifest",
  "services": [
    {
      "name": "vmm",
      "image": "/svc/vmm",
      "critical": false,
      "restart": "on-failure",
      "after": ["network"],
      "capabilities": [
        {
          "source": "network.primary",
          "purpose": "vmm.network-primary",
          "operation": "duplicate",
          "rights": ["read", "write"]
        }
      ]
    },
    {
      "name": "network",
      "image": "/svc/network-manager",
      "critical": true,
      "restart": "always",
      "after": [],
      "capabilities": []
    },
    {
      "name": "session",
      "image": "/svc/session-manager",
      "critical": true,
      "restart": "on-failure",
      "after": [],
      "capabilities": [
        {
          "source": "bootstrap.console-manager",
          "purpose": "session.console-manager",
          "operation": "create",
          "rights": ["inspect"]
        }
      ]
    }
  ]
}
"#;

#[test]
fn bootstrap_schema_rejects_formal_environment_controls() {
    for field in [r#""phase":"system","#, r#""loader":"dynamic","#] {
        let input = VALID.replacen(
            r#""critical": false,"#,
            &format!("{field} \"critical\": false,"),
            1,
        );
        assert_eq!(
            parse(&input).map_err(|e| e.kind()),
            Err(ParseErrorKind::InvalidEnum)
        );
    }
    let fleet = VALID.replacen(
        r#""services":"#,
        r#""virtual-machines":{"config":"/etc/vms.json"},"services":"#,
        1,
    );
    assert_eq!(
        parse(&fleet).map_err(|e| e.kind()),
        Err(ParseErrorKind::UnknownField)
    );
}

#[test]
fn service_phase_is_strict_and_cannot_be_redeclared() {
    for (field, kind) in [
        ("\"phase\": \"later\",", ParseErrorKind::InvalidEnum),
        (
            "\"phase\": \"boot\", \"phase\": \"boot\",",
            ParseErrorKind::DuplicateField,
        ),
    ] {
        let source = VALID.replacen(
            "\"critical\": false,",
            &format!("{field} \"critical\": false,"),
            1,
        );
        assert_eq!(parse(&source).map_err(|error| error.kind()), Err(kind));
    }
}

struct Policy;

const READ: u64 = 0b0000_0001;
const WRITE: u64 = 0b0000_0010;
const INSPECT_DETAILS: u64 = 1 << 33;
const INSPECT: u64 = 0b0000_0100;
const WAIT: u64 = 0b0000_1000;
const CREATE_PROCESS: u64 = 0b0001_0000;
const ATTACH_PROCESS: u64 = 0b0010_0000;
const SPONSOR: u64 = 0b0100_0000;
const DUPLICATE: u64 = 0b1000_0000;
const TRANSFER: u64 = 0b1_0000_0000;
const EXECUTE: u64 = 0b10_0000_0000;
const PROTECT: u64 = 1 << 35;
const SET_ATTRIBUTES: u64 = 1 << 31;
const LOCK_FILE: u64 = 1 << 32;
const CREATE_TASK_GROUP: u64 = 0b1_0000_0000_0000;
const CREATE_RESOURCE_DOMAIN: u64 = 0b10_0000_0000_0000;
const DERIVE: u64 = 0b0100_0000_0000_0000;
const CREATE_VIRTUAL_MACHINE: u64 = 0b1000_0000_0000_0000;
fn test_authority_key(source: &str) -> AuthorityKey {
    let value = source
        .bytes()
        .fold(0_u16, |hash, byte| hash.wrapping_mul(31) ^ u16::from(byte));
    AuthorityKey::new(value)
}

impl AuthorityPolicy for Policy {
    fn authority<'policy>(&'policy self, source: &str) -> Option<AuthorityDeclaration<'policy>> {
        match source {
            "network.primary" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: Some("network"),
                object_kind: 1,
                rights: 0b11,
                movable: true,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.console-manager" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 2,
                rights: 0b100,
                movable: true,
                duplicable: true,
                creatable: true,
            }),
            "bootstrap.console" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 3,
                rights: 0b1011,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.service-output-channel" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 4,
                rights: WAIT | WRITE | DUPLICATE | TRANSFER,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.shell-input-channel" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 4,
                rights: WAIT | READ | DUPLICATE | TRANSFER | INSPECT,
                movable: true,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.console-control-server" | "bootstrap.console-log-server" => {
                Some(AuthorityDeclaration {
                    key: test_authority_key(source),
                    provider: None,
                    object_kind: 22,
                    rights: WAIT | READ | WRITE | DUPLICATE | TRANSFER,
                    movable: true,
                    duplicable: false,
                    creatable: false,
                })
            }
            "bootstrap.console-controller" | "bootstrap.console-log-connector" => {
                Some(AuthorityDeclaration {
                    key: test_authority_key(source),
                    provider: None,
                    object_kind: 22,
                    rights: WAIT | READ | WRITE | DUPLICATE | TRANSFER,
                    movable: false,
                    duplicable: true,
                    creatable: false,
                })
            }
            "bootstrap.shell-output-channel" | "bootstrap.shell-error-channel" => {
                Some(AuthorityDeclaration {
                    key: test_authority_key(source),
                    provider: None,
                    object_kind: 4,
                    rights: WAIT | READ | WRITE | DUPLICATE | TRANSFER,
                    movable: true,
                    duplicable: false,
                    creatable: false,
                })
            }
            "bootstrap.root-directory" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 5,
                rights: READ
                    | WRITE
                    | INSPECT
                    | EXECUTE
                    | PROTECT
                    | DUPLICATE
                    | TRANSFER
                    | SET_ATTRIBUTES
                    | LOCK_FILE,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.environment-directory" | "bootstrap.boot-environment-directory" => {
                Some(AuthorityDeclaration {
                    key: test_authority_key(source),
                    provider: None,
                    object_kind: 5,
                    rights: READ | INSPECT | EXECUTE | PROTECT | DUPLICATE | TRANSFER,
                    movable: false,
                    duplicable: true,
                    creatable: false,
                })
            }
            "bootstrap.child-loader" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 21,
                rights: READ | EXECUTE | DUPLICATE | TRANSFER,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.dynamic-library-directory" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 5,
                rights: READ | EXECUTE | PROTECT | DUPLICATE | TRANSFER,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.task-factory" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 6,
                rights: 0b1_0000_0000_0000 | 0b001_0000 | 0b1000_0000 | 0b1_0000_0000,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.task-group" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 7,
                rights: ATTACH_PROCESS | DUPLICATE | TRANSFER,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.resource-domain" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 8,
                rights: 0b10_0000_0000_0000 | 0b100_0000 | 0b1000_0000 | 0b1_0000_0000,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.task-inspector" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 16,
                rights: 0b1_1000_0100,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.object-inspector" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 17,
                rights: 0b1_1000_0100 | INSPECT_DETAILS,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.memory-inspector" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 18,
                rights: 0b1_1000_0100,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.cpu-inspector" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 19,
                rights: 0b1_1000_0100 | INSPECT_DETAILS | (1 << 31),
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.vm-creation-authority" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 20,
                rights: 0b1100_0000_0000_0000 | 0b1000_0000 | 0b1_0000_0000,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            "bootstrap.vm-runtime-image" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 21,
                rights: READ | EXECUTE | PROTECT | DUPLICATE | TRANSFER,
                movable: false,
                duplicable: false,
                creatable: true,
            }),
            "bootstrap.vm-provisioning-channel" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 22,
                rights: WAIT | READ | WRITE | TRANSFER,
                movable: true,
                duplicable: false,
                creatable: false,
            }),
            "bootstrap.vm-manager-connection-channel" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 22,
                rights: WAIT | READ | TRANSFER,
                movable: true,
                duplicable: false,
                creatable: false,
            }),
            "bootstrap.vm-client-connection-channel" => Some(AuthorityDeclaration {
                key: test_authority_key(source),
                provider: None,
                object_kind: 22,
                rights: WAIT | READ | WRITE | DUPLICATE | TRANSFER,
                movable: false,
                duplicable: true,
                creatable: false,
            }),
            _ => None,
        }
    }

    fn startup_purpose(&self, image: &str, name: &str) -> Option<StartupPurposeDeclaration> {
        let (value, object_kind, required_rights) = match (image, name) {
            ("/svc/vmm", "vmm.network-primary") => (100, 1, READ | WRITE),
            ("/svc/session-manager", "session.console-manager")
            | ("/svc/session-manager", "session.console-manager-alias") => (100, 2, INSPECT),
            ("/svc/network-manager", "network.console-manager") => (101, 2, INSPECT),
            ("/svc/console-raw", "console.system") => (200, 3, WAIT | READ | WRITE),
            ("/svc/console-raw", "console.control") => (201, 22, WAIT | READ),
            ("/svc/console-raw", "console.logs") => (202, 22, WAIT | READ),
            ("/svc/session-manager", "session.console-controller") => {
                (203, 22, WAIT | WRITE | DUPLICATE | TRANSFER)
            }
            ("/svc/session-manager", "session.log-connector") => {
                (204, 22, WAIT | WRITE | DUPLICATE | TRANSFER)
            }
            ("/svc/session-manager", "process.task-factory") => (
                304,
                6,
                CREATE_PROCESS | CREATE_TASK_GROUP | DUPLICATE | TRANSFER,
            ),
            ("/bin/sh", "stdio.input") => (300, 4, WAIT | READ | DUPLICATE | TRANSFER | INSPECT),
            ("/bin/sh" | "/svc/session-manager" | "/svc/vm-manager", "stdio.output") => {
                (301, 4, WAIT | WRITE | DUPLICATE | TRANSFER)
            }
            ("/bin/sh" | "/svc/session-manager" | "/svc/vm-manager", "stdio.error") => {
                (302, 4, WAIT | WRITE | DUPLICATE | TRANSFER)
            }
            ("/svc/vm-manager", "process.root-directory") => (303, 5, READ | DUPLICATE | TRANSFER),
            ("/bin/sh" | "/svc/session-manager", "process.root-directory") => (
                303,
                5,
                READ | WRITE
                    | INSPECT
                    | DUPLICATE
                    | TRANSFER
                    | EXECUTE
                    | PROTECT
                    | SET_ATTRIBUTES
                    | LOCK_FILE,
            ),
            ("/bin/sh", "process.task-factory") => (304, 6, CREATE_PROCESS | DUPLICATE | TRANSFER),
            ("/bin/sh" | "/svc/session-manager", "process.task-group") => {
                (305, 7, ATTACH_PROCESS | DUPLICATE | TRANSFER)
            }
            ("/bin/sh" | "/svc/session-manager", "process.resource-domain") => {
                (306, 8, SPONSOR | DUPLICATE | TRANSFER)
            }
            ("/bin/sh" | "/svc/session-manager", "process.task-inspector") => {
                (307, 16, DUPLICATE | TRANSFER | INSPECT)
            }
            ("/bin/sh" | "/svc/session-manager", "process.object-inspector") => {
                (308, 17, DUPLICATE | TRANSFER | INSPECT | INSPECT_DETAILS)
            }
            ("/bin/sh" | "/svc/session-manager", "process.memory-inspector") => {
                (309, 18, DUPLICATE | TRANSFER | INSPECT)
            }
            ("/bin/sh" | "/svc/session-manager", "process.cpu-inspector") => (
                310,
                19,
                DUPLICATE | TRANSFER | INSPECT | INSPECT_DETAILS | (1 << 31),
            ),
            ("/bin/sh" | "/svc/session-manager", "process.environment-directory") => (
                317,
                5,
                READ | INSPECT | EXECUTE | PROTECT | DUPLICATE | TRANSFER,
            ),
            ("/bin/sh" | "/svc/session-manager" | "/svc/vm-manager", "process.child-loader") => {
                (316, 21, READ | EXECUTE | DUPLICATE | TRANSFER)
            }
            ("/bin/sh" | "/svc/session-manager", "process.child-library-directory")
            | ("/svc/vm-manager", "process.child-library-directory") => {
                (315, 5, READ | EXECUTE | PROTECT | DUPLICATE | TRANSFER)
            }
            ("/svc/session-manager", "vm.manager-connection") => (
                crate::contracts::vm::MANAGER_CONNECTION.as_raw(),
                22,
                WAIT | WRITE | DUPLICATE | TRANSFER,
            ),
            ("/bin/sh", "vm.manager-connection") => (
                crate::contracts::vm::MANAGER_CONNECTION.as_raw(),
                22,
                WAIT | WRITE,
            ),
            ("/svc/vm-manager", "vm.runtime-image") => {
                (312, 21, READ | EXECUTE | PROTECT | DUPLICATE | TRANSFER)
            }
            ("/svc/vm-manager", "vm.provisioning") => {
                (MOCK_VM_PROVISIONING_PURPOSE, 22, WAIT | READ)
            }
            ("/svc/vm-manager", "vm.manager-connection") => (
                crate::contracts::vm::MANAGER_CONNECTION.as_raw(),
                22,
                WAIT | READ,
            ),
            ("/svc/vm-manager", "process.task-factory") => {
                (304, 6, CREATE_PROCESS | CREATE_TASK_GROUP)
            }
            ("/svc/vm-manager", "process.resource-domain") => (306, 8, CREATE_RESOURCE_DOMAIN),
            ("/svc/vm-manager", "vm.creation-authority") => {
                (311, 20, DERIVE | CREATE_VIRTUAL_MACHINE)
            }
            _ => return None,
        };
        Some(StartupPurposeDeclaration {
            value,
            object_kind,
            required_rights,
            allowed_rights: required_rights,
        })
    }

    fn right(&self, name: &str) -> Option<u64> {
        match name {
            "read" => Some(READ),
            "write" => Some(WRITE),
            "inspect" => Some(INSPECT),
            "inspect-details" => Some(INSPECT_DETAILS),
            "wait" => Some(WAIT),
            "create-process" => Some(CREATE_PROCESS),
            "attach-process" => Some(ATTACH_PROCESS),
            "sponsor" => Some(SPONSOR),
            "duplicate" => Some(DUPLICATE),
            "transfer" => Some(TRANSFER),
            "execute" => Some(EXECUTE),
            "protect" => Some(PROTECT),
            "set-attributes" => Some(SET_ATTRIBUTES),
            "lock-file" => Some(LOCK_FILE),
            "create-task-group" => Some(CREATE_TASK_GROUP),
            "create-resource-domain" => Some(CREATE_RESOURCE_DOMAIN),
            "derive" => Some(DERIVE),
            "create-virtual-machine" => Some(CREATE_VIRTUAL_MACHINE),
            _ => None,
        }
    }
}

struct CollidingPolicy;

impl AuthorityPolicy for CollidingPolicy {
    fn authority<'policy>(&'policy self, source: &str) -> Option<AuthorityDeclaration<'policy>> {
        let declaration = Policy.authority(source)?;
        Some(AuthorityDeclaration {
            key: AuthorityKey::new(7),
            ..declaration
        })
    }

    fn startup_purpose(&self, image: &str, name: &str) -> Option<StartupPurposeDeclaration> {
        Policy.startup_purpose(image, name)
    }

    fn right(&self, name: &str) -> Option<u64> {
        Policy.right(name)
    }
}

#[test]
fn composed_boot_manifest_matches_its_production_policy() -> Result<(), String> {
    let text = boot_projection(include_str!("../config/services-with-vms.json"))?;
    let manifest = parse(&text).map_err(|e| format!("{e:?}"))?;
    let plan = validate(&manifest, &crate::bootstrap_policy::BootstrapPolicy)
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(plan.service_count(), 2);
    assert!(
        manifest
            .services()
            .all(|service| matches!(service.name(), "console-raw" | "io-runtime"))
    );
    Ok(())
}

#[test]
fn singleton_role_lookup_rejects_an_ambiguous_purpose() {
    let parsed = parse(VALID);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    let validated = validate(&manifest, &Policy);
    assert!(validated.is_ok());
    let Ok(plan) = validated else {
        return;
    };
    assert_eq!(plan.unique_service_for_purpose(100), None);
    assert_eq!(plan.unique_service_for_purpose(0), None);
}

#[test]
fn composed_boot_console_remains_critical() -> Result<(), String> {
    let text = boot_projection(include_str!("../config/services-console-only.json"))?;
    let manifest = parse(&text).map_err(|e| format!("{e:?}"))?;
    assert_eq!(manifest.service_count(), 1);
    assert!(
        manifest
            .service(0)
            .is_some_and(|service| service.name() == "console-raw" && service.critical())
    );
    Ok(())
}

#[test]
fn parses_and_plans_a_complete_manifest_before_launch() {
    let parsed = parse(VALID);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    let validated = validate(&manifest, &Policy);
    assert!(validated.is_ok());
    let Ok(plan) = validated else {
        return;
    };
    assert_eq!(manifest.service_count(), 3);
    assert_eq!(plan.service_count(), 3);
    assert_eq!(plan.service_index(0), Some(1));
    assert_eq!(plan.service_index(1), Some(0));
    assert_eq!(plan.service_index(2), Some(2));
    let first = plan.capability_grant(0, 0);
    assert!(first.is_some());
    let Some(first) = first else {
        return;
    };
    assert_eq!(first.rights(), 0b11);
    assert_eq!(first.object_kind(), 1);
    assert_eq!(first.purpose(), 100);
    assert_eq!(first.authority(), test_authority_key("network.primary"));
    assert_eq!(
        plan.capability_grant(2, 0)
            .map(super::CapabilityGrant::object_kind),
        Some(2)
    );
    assert_eq!(
        plan.capability_grant(2, 0)
            .map(super::CapabilityGrant::purpose),
        Some(100)
    );
}

#[test]
fn rejects_authority_key_reuse_for_distinct_sources() {
    let parsed = parse(VALID);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &CollidingPolicy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::ConflictingAuthorityKey)
    );
}

#[test]
fn rejects_unknown_and_duplicate_fields() {
    let unknown = VALID.replace("\"critical\": false", "\"enabled\": false");
    let parsed = parse(&unknown);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.kind(), ParseErrorKind::UnknownField);
    assert_eq!(error.offset(), unknown.find("\"enabled\"").unwrap_or(0));

    let duplicate = VALID.replace(
        "\"critical\": false,",
        "\"critical\": false, \"critical\": false,",
    );
    assert_eq!(
        parse(&duplicate).map_err(|error| error.kind()),
        Err(ParseErrorKind::DuplicateField)
    );
}

#[test]
fn rejects_noncanonical_escaped_strings() {
    let escaped = VALID.replace("\"vmm\"", "\"v\\u006d\"");
    assert_eq!(
        parse(&escaped).map_err(|error| error.kind()),
        Err(ParseErrorKind::EscapeNotAllowed)
    );
}

#[test]
fn enforces_manifest_and_service_count_bounds() {
    let oversized = " ".repeat(MAX_MANIFEST_BYTES + 1);
    assert_eq!(
        parse(&oversized).map_err(|error| error.kind()),
        Err(ParseErrorKind::TooLarge)
    );

    let mut many = String::from("{\"format\":\"hyper.service-manifest\",\"services\":[");
    for index in 0..=MAX_SERVICES {
        if index != 0 {
            many.push(',');
        }
        let write_result = write!(
            many,
            "{{\"name\":\"s{index}\",\"image\":\"/svc/s{index}\",\"critical\":false,\"restart\":\"never\",\"after\":[],\"capabilities\":[]}}"
        );
        assert!(write_result.is_ok());
    }
    many.push_str("]}");
    assert_eq!(
        parse(&many).map_err(|error| error.kind()),
        Err(ParseErrorKind::TooManyServices)
    );
}

#[test]
fn rejects_dependency_cycles() {
    let cyclic = r#"{
      "format":"hyper.service-manifest",
      "services":[
        {"name":"a","image":"/svc/a","critical":true,"restart":"never","after":["b"],"capabilities":[]},
        {"name":"b","image":"/svc/b","critical":true,"restart":"never","after":["a"],"capabilities":[]}
      ]
    }"#;
    let parsed = parse(cyclic);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::DependencyCycle)
    );
}

#[test]
fn rejects_rights_escalation() {
    let escalated = VALID.replace(
        "\"rights\": [\"inspect\"]",
        "\"rights\": [\"inspect\", \"write\"]",
    );
    let parsed = parse(&escalated);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::RightsEscalation)
    );
}

#[test]
fn rejects_rights_below_the_destination_contract() {
    let under_delegated = VALID.replace(
        "\"rights\": [\"read\", \"write\"]",
        "\"rights\": [\"read\"]",
    );
    let parsed = parse(&under_delegated);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    let error = validate(&manifest, &Policy);
    assert_eq!(
        error.map_err(|error| (error.kind(), error.service(), error.capability())),
        Err((ValidationErrorKind::MissingRequiredRights, Some(0), Some(0)))
    );
}

#[test]
fn rejects_rights_above_the_destination_contract() -> Result<(), String> {
    let source = boot_projection(include_str!("../config/services-console-only.json"))?;
    let production = source.split_whitespace().collect::<String>();
    let over_delegated =
        production.replacen("[\"wait\",\"read\"]", "[\"wait\", \"read\", \"write\"]", 1);
    let parsed = parse(&over_delegated);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return Err("over-delegated fixture failed parsing".into());
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| (
            error.kind(),
            error.service(),
            error.capability()
        )),
        Err((ValidationErrorKind::ExcessPurposeRights, Some(0), Some(1),))
    );
    Ok(())
}

#[test]
fn rejects_hidden_provider_dependencies() {
    let hidden = VALID.replace("\"after\": [\"network\"]", "\"after\": []");
    let parsed = parse(&hidden);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::MissingProviderDependency)
    );
}

#[test]
fn rejects_one_shot_authority_for_a_restartable_service() {
    let one_shot = VALID.replace("\"operation\": \"create\"", "\"operation\": \"move\"");
    let parsed = parse(&one_shot);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::RestartConsumesAuthority)
    );
}

#[test]
fn rejects_reuse_of_a_moved_authority() {
    let repeated = VALID
        .replace(
            "\"capabilities\": []",
            r#""capabilities": [{
          "source":"bootstrap.console-manager",
          "purpose":"network.console-manager",
          "operation":"duplicate",
          "rights":["inspect"]
        }]"#,
        )
        .replace("\"operation\": \"create\"", "\"operation\": \"move\"");
    let parsed = parse(&repeated);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::MoveSourceReused)
    );
}

#[test]
fn rejects_invalid_and_duplicate_startup_purposes() {
    let invalid = VALID.replacen(
        "\"purpose\": \"vmm.network-primary\"",
        "\"purpose\": \"0bad\"",
        1,
    );
    let parsed = parse(&invalid);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::InvalidPurposeName)
    );

    let duplicate = VALID.replace(
        r#""rights": ["inspect"]"#,
        r#""rights": ["inspect"]
        }, {
          "source": "bootstrap.console-manager",
          "purpose": "session.console-manager-alias",
          "operation": "duplicate",
          "rights": ["inspect"]"#,
    );
    let parsed = parse(&duplicate);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::DuplicateCapabilityPurpose)
    );

    let unknown = VALID.replacen(
        "\"purpose\": \"vmm.network-primary\"",
        "\"purpose\": \"vmm.unknown\"",
        1,
    );
    let parsed = parse(&unknown);
    assert!(parsed.is_ok());
    let Ok(manifest) = parsed else {
        return;
    };
    assert_eq!(
        validate(&manifest, &Policy).map_err(|error| error.kind()),
        Err(ValidationErrorKind::UnknownCapabilityPurpose)
    );
}

#[test]
fn composed_native_boot_manifest_needs_only_raw_console() -> Result<(), String> {
    let text = boot_projection(include_str!("../config/services-console-only.json"))?;
    let manifest = parse(&text).map_err(|e| format!("{e:?}"))?;
    let plan = validate(&manifest, &crate::bootstrap_policy::BootstrapPolicy)
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(plan.service_count(), 1);
    Ok(())
}

#[test]
fn optional_declarations_cannot_select_a_dynamic_bootstrap_loader() {
    assert!(parse(VALID).is_ok());
    let declared = VALID.replacen(
        r#""critical": false,"#,
        r#""loader":"bootstrap","phase":"boot","critical": false,"#,
        1,
    );
    assert!(parse(&declared).is_ok());
    for replacement in ["auto", "dynamic"] {
        let invalid = declared.replace(
            r#""loader":"bootstrap""#,
            &format!("\"loader\":\"{replacement}\""),
        );
        assert_eq!(
            parse(&invalid).map_err(|e| e.kind()),
            Err(ParseErrorKind::InvalidEnum)
        );
    }
}

fn boot_projection(source: &str) -> Result<String, String> {
    let mut manifest: serde_json::Value =
        serde_json::from_str(source).map_err(|e| e.to_string())?;
    let object = manifest
        .as_object_mut()
        .ok_or("manifest is not an object")?;
    object.remove("virtual-machines");
    let services = object
        .get_mut("services")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("missing services")?;
    services.retain(|service| service["loader"] == "bootstrap");
    for service in services {
        service["phase"] = "boot".into();
    }
    serde_json::to_string(&manifest).map_err(|e| e.to_string())
}
