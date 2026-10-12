// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn sparse_policy_preserves_vcpu_and_mask_word_boundaries() {
    let entries = [Affinity {
        vcpu: 3,
        cpus: vec![0, 63, 64, 255],
    }];
    assert_eq!(
        masks(&entries, 4).unwrap(),
        [(3, [1 | (1 << 63), 1, 0, 1 << 63])]
    );
    assert!(masks(&[], 4).unwrap().is_empty());
}

#[test]
fn invalid_policy_has_no_placement_side_effects() {
    let valid = Affinity {
        vcpu: 0,
        cpus: vec![1],
    };
    for invalid in [
        Affinity {
            vcpu: 2,
            cpus: vec![0],
        },
        Affinity {
            vcpu: 0,
            cpus: vec![0],
        },
        Affinity {
            vcpu: 1,
            cpus: vec![],
        },
        Affinity {
            vcpu: 1,
            cpus: vec![2, 2],
        },
        Affinity {
            vcpu: 1,
            cpus: vec![MAX_HOST_CPUS as u32],
        },
    ] {
        let mut calls = 0;
        assert!(
            apply(&[valid.clone(), invalid], 2, |_, _| {
                calls += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(calls, 0);
    }
}

#[test]
fn rejected_host_mask_aborts_startup_policy() {
    let entries = [
        Affinity {
            vcpu: 0,
            cpus: vec![1],
        },
        Affinity {
            vcpu: 1,
            cpus: vec![0],
        },
    ];
    let mut calls = Vec::new();
    let error = apply(&entries, 2, |vcpu, words| {
        calls.push((vcpu, words.to_vec()));
        Err(hyper_os::Error::InvalidResponse)
    })
    .unwrap_err();
    assert!(error.contains("default affinity for vCPU 0 rejected"));
    assert_eq!(calls, [(0, vec![2, 0, 0, 0])]);
}

#[test]
fn omitted_affinity_does_not_change_scheduler_placement() {
    apply(&[], 2, |_, _| panic!("no placement requested")).unwrap();
}
