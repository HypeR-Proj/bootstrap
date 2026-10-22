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

The default bootstrap graph starts the raw console, I/O runtime and filesystem manager.
They read configuration through fixed bootstrap-environment capabilities. The
bootstrap fstab mounts the independent system filesystem temporarily at `/system`.
The system payload supplies `/boot`, `/tmp` and `/data` mountpoint directories.
Stage-1 reads `/etc/hyper/boot-profiles/default.json`, or the profile selected by
the firmware argument `hyper.boot-profile=NAME`:

```json
{
  "version": 1,
  "services": "/etc/hyper/services.json",
  "io": {"kind": "io-vm", "service": "io-runtime", "successor": "/io-runtime"},
  "handoff": {"system-root": "/system", "put-old": "/boot", "system-init": "/svc/component-manager"},
  "requires": ["/vm/io.itb"]
}
```

Stage-1 provisionally switches the namespace before starting system init.
The system filesystem is now `/`, and
bootstrap remains at `/boot`. No compatibility aliases or permanent `/system`
wrapper are installed. The successor reads its own `/etc/hyper/components.json`:

```json
{"configuration_directory":"/etc/components","root":"root","startup":"/startup"}
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
produce mount errors when selected by media probing. Explicit profile `requires`
dependencies and all bootstrap service executables are checked against the boot
archive during packaging, for every included profile. The generated default
also declares the worker required by the board's system filesystem.

Profiles can select `io.kind: "service"` for a different storage provider, or
`"none"` for recovery or an independently established root. No I/O VM launches
unless selected. A provider participating in handoff names a formal successor;
formal startup policy must admit it and satisfy the capabilities its services
require. The generic coordinator forwards the storage migration channel without
parsing an I/O VM description. See [boot profiles](../../../../docs/boot-layout.md#boot-profiles).

The formal graph starts `session-manager`, which creates `terminal-service` and
`bin/sh`. `/etc/hyper/session.json` optionally selects relative terminal/shell
paths and a rapid-failure restart limit. The fallback shell handles only failures
before handoff; formal failures never restart it. See
[Native terminals](../../../../docs/terminal.md).

The default `vm-manager` is noncritical and independent of I/O VM launch. Omitting
business VMs does not change bootstrap storage or the handoff protocol. Formal
VM fleet settings are loaded from `/data/vms.json` after the data volume mounts.
