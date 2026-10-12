<!-- SPDX-FileCopyrightText: 2026 roolrz -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Init configuration

`services-with-vms.json` includes the raw Console service, formal session manager,
VM manager and I/O runtime. Board packaging reads
`boards/<board>.json` and generates the installed manifest, including storage
readiness and broker endpoints. Edit the board configuration for VM definitions;
its generated `vms.json` lives on the configuration volume at `/data`.

The board's `io-vm` object is also the sole source of I/O VM settings, including
its runtime, name, image path, configuration and device selector. Board and
standalone I/O builds install a board snapshot at `/etc/hyper/board.json`, which
io-runtime reads directly. Diskless bring-up derives an ordinary VM definition
from the selected Pi 5 board for manual startup without device authority.

`services-console-only.json` includes the raw Console service and formal session
manager, used for early hardware bring-up and clock tests. These builds omit
`vms.json`; an empty VM configuration file is not needed.

Composition splits the selected manifest into a core graph, initially installed
at `/etc/hyper/services.json`, and a system graph under `/system`.

Test manifests live in `../tests/config/`. They may autostart test guests and are
not the production board configuration.

The boot graph starts `console-raw`, a byte-only transport with independent log
ingress. Stage-1 init launches and supervises `svc/fallback-sh` directly only
when startup fails before handoff. The system graph starts `session-manager`,
which launches `terminal-service` and the formal shell. No shared startup-decision
channel selects between recovery and formal shells. After handoff, formal init
owns errors; missing system tools cannot reactivate bootstrap recovery.

The default manager reads optional `/system/etc/hyper/session.json`. Its relative
`terminal` and `shell` image paths default to `svc/terminal-service` and `bin/sh`;
`restart_limit` defaults to three rapid failures. It supervises a fresh session
on restart and owns only its delegated raw-console control endpoint. Current
board deployment selects one physical Console. See
[Native terminals](../../../../docs/terminal.md).

## Stage-1 and the system environment

The RAM archive initially contains core services and configuration directly
under `/`; there is no prepacked `/boot` wrapper. The system graph lives at
`/system/etc/hyper/services.json`. `/etc/hyper/init.json` selects only the
successor executable: `{"system-init": "/system/svc/init-system"}`. The
successor reads its own `/system/etc/hyper/init.json`; the default split
deployment generates:

```json
{
  "services": "/system/etc/hyper/services.json",
  "root-switch": {
    "new-root": "/.newroot",
    "put-old": "/boot",
    "directories": ["/tmp", "/data"],
    "mounts": [
      {"source": "/system", "target": "/system", "ramfs": false}
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
handoff when `/system` is available, including diskless ramfs deployment.
After ownership commits, formal init handles failures and uses the formal shell.
After adoption, system init prepares a fresh ramfs root and atomically switches
the namespace, retaining the old root at `/boot` and moving the listed mounts.
Core configuration is then reachable at `/boot/etc/hyper/`. The formal root
has no compatibility aliases; services read boot resources through their fixed
`process.environment-directory`, separately from the live namespace root. A root-switch failure leaves the old namespace intact; system init keeps
the transferred core supervisors and handles recovery in the formal environment. Supervisor transfer and root replacement are distinct commit points.

Choosing ramfs preserves this graph, handoff and root switch; the `/system`
entry instead sets `ramfs: true` to promote and detach the resident subtree atomically without copying it.
Additional bootstrap fstab mountpoints are included as optional moves. Formal
mountpoints are created later and do not constrain the root-switch plan. Nested disk mounts
follow a moved ancestor; mounted children under a RAM subtree transfer are
rejected. Ordinary services and the shell do not receive `MANAGE_NAMESPACE`.
This policy belongs to userspace: omitting `root-switch` from a custom init
configuration performs the supervision handoff without replacing the root.

`/boot/etc/fstab` is the retained bootstrap mount policy, containing only `/system`
by default. After handoff and root switching, formal init requests the mounts in
`/system/etc/fstab`, including `/data`. `/boot/etc/hyper/filesystem.json` configures
only filesystem-worker search directories. The default FAT and EROFS workers live in boot RAM; worker search can also
resolve implementations installed under `/system/svc`. Partition selections come from the
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
