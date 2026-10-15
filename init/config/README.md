<!-- SPDX-FileCopyrightText: 2026 roolrz -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Init configuration

The board configuration generates separate bootstrap and formal service graphs.
`services-with-vms.json` supplies the ordinary composition; `services-console-only.json`
is used by explicit hardware bring-up fixtures. Test-specific manifests live in
`../tests/config/`. Edit `boards/<board>.json` for production VM and storage policy.

The composed bootstrap graph contains only static services. Its parser accepts
only `phase: "boot"` and `loader: "bootstrap"` when those fields are present;
it has no VM-fleet configuration or formal-service launch path. The mixed source
templates above must be composed before deployment.

The bootstrap graph starts the raw console, I/O runtime and filesystem manager.
They read configuration through fixed bootstrap-environment capabilities. The
bootstrap fstab mounts the independent system filesystem temporarily at `/system`.
The system payload supplies `/boot`, `/tmp` and `/data` mountpoint directories.
Stage-1 reads this policy from its own `/etc/hyper/init.json`:

```json
{"system-root":"/system","put-old":"/boot","system-init":"/svc/component-manager"}
```

Stage-1 provisionally switches the namespace before starting system init.
The system filesystem is now `/`, and
bootstrap remains at `/boot`. No compatibility aliases or permanent `/system`
wrapper are installed. The successor reads its own `/etc/hyper/init.json`:

```json
{"services":"/etc/hyper/services.json"}
```

The candidate validates its graph, locks current mappings, and receives Process
supervisors and the pending root-transition token in one capability rendezvous.
System init commits that transition and mounts a fresh RAMFS at `/tmp` before
starting replacement providers or formal services.
Before that commit, failures restore the original namespace and stage-1 starts
`svc/fallback-sh`. After commit, system init owns failures, service replacement
and retirement. Stage-1 does not choose its own exit time. See the complete
[handoff contract](../../../../docs/bootstrap-handoff.md).

Bootstrap components use the static-only userspace loader. Formal components
use the dynamic loader, including static executables. Their library and worker
implementations are independent. System init first adopts the live I/O VM and
existing filesystem transports, then reads its own `/etc/fstab`, including the
`/data` mount. GPT partition selectors are resolved by HypeR; format workers are
selected from media magic and configured worker directories. Missing workers
produce mount errors rather than hidden build-time filesystem dependencies.

The formal graph starts `session-manager`, which creates `terminal-service` and
`bin/sh`. `/etc/hyper/session.json` optionally selects relative terminal/shell
paths and a rapid-failure restart limit. The fallback shell handles only failures
before handoff; formal failures never restart it. See
[Native terminals](../../../../docs/terminal.md).

The default `vm-manager` is noncritical and independent of I/O VM launch. Omitting
business VMs does not change bootstrap storage or the handoff protocol. Formal
VM fleet settings are loaded from `/data/vms.json` after the data volume mounts.
