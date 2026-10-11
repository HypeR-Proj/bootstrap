<!-- SPDX-FileCopyrightText: 2026 roolrz -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Init configuration

`services-with-vms.json` includes the console workers, session manager,
VM manager and I/O runtime. Board packaging reads
`boards/<board>.json` and generates the installed manifest, including storage
readiness and broker endpoints. Edit the board configuration for VM definitions;
its generated `vms.json` lives on the configuration volume at `/data`.

The board's `io-vm` object is also the sole source of I/O VM settings, including
its runtime, name, image path, configuration and device selector. Board and
standalone I/O builds install a board snapshot at `/etc/hyper/board.json`, which
io-runtime reads directly. Diskless bring-up derives an ordinary VM definition
from the selected Pi 5 board for manual startup without device authority.

`services-console-only.json` includes only the console workers and session
manager, used for early hardware bring-up and clock tests. These builds omit
`vms.json`; an empty VM configuration file is not needed.

Composition splits the selected manifest into a core graph, initially installed
at `/etc/hyper/services.json`, and a system graph under `/system`.

Test manifests live in `../tests/config/`. They may autostart test guests and are
not the production board configuration.

Init supervises the critical `session` virtual console manager, not the shell directly.
The manager establishes its console but does not read input or start a shell
until init sends `session.startup-decision`. That channel's sole writer follows
supervisor ownership across handoff. Successful initialization selects
`/system/bin/sh`; failure selects the fixed boot environment's `svc/fallback-sh`.
A missing/failed formal shell, closed decision channel or 180-second decision
timeout also enters recovery. Clean shell exit restarts the selected client;
rapid failures are rate-limited. Console transport failure remains a service
failure. Each manager owns only its assigned console channels; the current
bootstrap wires one physical console.

## Stage-1 and the system environment

The RAM archive initially contains core services and configuration directly
under `/`; there is no prepacked `/boot` wrapper. The system graph lives at
`/system/etc/hyper/services.json`. `/etc/hyper/init.json` selects system init
and its root-switch policy. The default split deployment generates:

```json
{
  "system-init": "/system/svc/init-system",
  "services": "/system/etc/hyper/services.json",
  "root-switch": {
    "new-root": "/.newroot",
    "put-old": "/boot",
    "directories": ["/tmp"],
    "mounts": [
      {"source": "/system", "target": "/system", "ramfs": false},
      {"source": "/data", "target": "/data", "optional": true}
    ]
  }
}
```

Boot services have `phase: "boot"`; system services have `phase: "system"`.
Loader selection is a separate userspace field: `loader: "bootstrap"` selects
the static-only core loader, while `loader: "dynamic"` (the default) selects
the formal loader, which also accepts static executables. Services which
construct children receive `process.child-loader` from `bootstrap.child-loader`
with `execute`, `duplicate` and `transfer` rights. It is not implicitly granted
to services that only run their own image.
The composer derives the phase from loader selection and rejects a conflicting
explicit phase. System graphs may depend on existing core services,
but cannot redefine their identities or reverse the startup-phase order. The
system init validates the combined graph before accepting supervisor
ownership. One successful capability rendezvous transfers all live core
supervisors; only then does boot init exit. An unavailable system image or its
library directory, invalid configuration or failed pre-commit transfer leaves
recovery supervised by boot init. An unrelated `/data` failure does not prevent
handoff when `/system` is available, including diskless ramfs deployment, but
the failure follows the handoff and selects the recovery shell.
After adoption, system init prepares a fresh ramfs root and atomically switches
the namespace, retaining the old root at `/boot` and moving the listed mounts.
Core configuration is then reachable at `/boot/etc/hyper/`. The formal root
has no compatibility aliases; services read boot resources through their fixed
`process.environment-directory`, separately from the live namespace root. A root-switch failure leaves the old namespace intact; system init keeps
the transferred core supervisors and recovery shell but does not launch formal
services. Supervisor transfer and root replacement are distinct commit points.

Choosing ramfs preserves this graph, handoff and root switch; the `/system`
entry instead sets `ramfs: true` to promote and detach the resident subtree atomically without copying it.
Additional fstab mountpoints are included as optional moves. Nested disk mounts
follow a moved ancestor; mounted children under a RAM subtree transfer are
rejected. Ordinary services and the shell do not receive `MANAGE_NAMESPACE`.
This policy belongs to userspace: omitting `root-switch` from a custom init
configuration performs the supervision handoff without replacing the root.

`/boot/etc/fstab` is the retained mount policy, and `/boot/etc/hyper/filesystem.json` configures
only filesystem-worker search directories. Partition selections come from the
board's generated boot-environment `etc/hyper/storage.json`; HypeR resolves
PARTUUID/PARTLABEL from GPT before authorizing Linux block ranges; implementations are selected
by media magic and loaded as separate workers. The default deployment mounts
read-only EROFS at `/system` and writable FAT at `/data`. Omitting a worker is
allowed and causes a clear runtime mount error. See
[boot layout](../../../../docs/boot-layout.md) and
[filesystem services](../../../../docs/filesystem-services.md) for recovery tools,
permissions and supported configuration.

The default `vm-manager` service is noncritical and io-runtime has no launch
dependency on it. To deploy Native storage without business VMs, omit the
manager service and the `virtual-machines` configuration. VM creation authority
for io-runtime is independent of the lazily created business fleet scope.
Stage-1 starts I/O before transferring supervision to the
system init; the latter may omit vm-manager without changing the core graph.
