<!--
SPDX-FileCopyrightText: 2026 roolrz
SPDX-License-Identifier: Apache-2.0
-->

# Boot profiles and mount policy

## Boot profiles

The archive contains `/etc/hyper/boot-profiles/default.json` and may contain
other named profiles. With no selector, stage-1 uses `default`. A firmware or
QEMU kernel command-line argument selects another profile:

```text
hyper.boot-profile=recovery
```

Names contain 1–63 ASCII letters, digits, underscores or hyphens. Missing,
malformed or repeated explicit selectors fail instead of falling back to
`default`. Boot arguments are consumed one stage at a time. The kernel handles
`earlycon` and `nokaslr`, then supplies the remaining arguments in an immutable
`BOOT_COMMAND_LINE` startup VMO. Stage-1 consumes `hyper.boot-profile` and gives
its remainder to system init through the same startup purpose. The kernel does
not parse profiles, launch I/O providers or decide root layout.

Arguments are separated by ASCII whitespace; there is no shell quoting or
expansion. Each stage preserves unrecognized argument bytes, order and duplicate
keys while normalizing separators to spaces. Invalid recognized profile selectors
fail in stage-1 rather than being forwarded. Component-manager currently defines
no boot arguments: it reports any remainder once and continues booting. Boot
arguments are separate from process `argv` and environment variables.

A profile selects a service manifest, I/O source and optional root handoff:

```json
{
  "version": 1,
  "services": "/etc/hyper/services.json",
  "io": {
    "kind": "io-vm",
    "service": "io-runtime",
    "successor": "/io-runtime"
  },
  "handoff": {
    "system-root": "/system",
    "put-old": "/boot",
    "system-init": "/svc/component-manager"
  },
  "requires": ["/vm/io.itb"]
}
```

`io.kind` is `io-vm`, `service` or `none`. The selected `service` is a name in
that profile's bootstrap manifest, while `successor` is a component instance in
the formal environment's admitted provider policy. Only an `io-vm` provider
receives VM-creation authority. A `service` provider can implement a userspace
storage driver without starting an I/O VM. This is a provider contract, not an
additional physical disk driver shipped by the default product. `none` starts
no storage provider; it is useful for recovery or a root prepared independently.
The chosen manifest determines which processes actually start. A profile without
`handoff` retains bootstrap supervision and recovery instead of starting system
init.

Product `boot-profiles` configuration defines the packaged profiles; omitted
configuration generates the default from the selected bootstrap services. Every
profile, including non-default ones, is checked against the actual boot archive:
its manifest, service executables, required files and configured worker paths
must be present there, and bootstrap executables must be static. Files existing
only in the build tree or formal system image do not satisfy boot dependencies.
I/O VM image preparation is needed only if at least one packaged profile selects
`io-vm`. The formal environment independently defines permitted successors and
their capability requirements; choosing a native provider cannot satisfy a
formal service that explicitly requires the I/O VM broker.
Each provider process has one handoff role: the selected storage provider cannot
also own `console.system` or `filesystem.attach`. Keep these independently
supervised services separate. Multiple storage replacement candidates may be
declared in the formal policy; only the candidate selected by the actual offered
storage owner starts.

`system-root` is resolved before the switch; `put-old` and `system-init` name
locations in the new namespace. Formal `/etc/hyper/components.json` selects the
component configuration directory, root declaration and startup coordinator; it
does not specify another root switch. See
[service management](https://hyper-proj.github.io/component-manager/docs/components.html) for the JSON schema.

Namespace mutation requires `MANAGE_NAMESPACE | WRITE | EXECUTE` on a namespace
root Directory. Ordinary shells and format workers do not receive that authority.
Stage-1 recovery can repair mounts, but does not automatically repeat the handoff;
there is no `initctl` retry command. A subsequent system startup attempt requires
a reboot. Critical failures after committed handoff follow formal supervisor policy.

Formal `session-manager` supervises `terminal-service` and `/bin/sh`. The static
`svc/fallback-sh` is a stage-1-only recovery path without clap, history, pipelines
or redirection. Neither shell searches `svc` for commands. Background services
receive independent bounded log channels; the raw console mirrors these logs
alongside interactive output. Replacing raw console does not add terminal policy
to bootstrap.

## Runtime filesystem policy

Each environment owns a separate fstab. The archive's `/etc/fstab` mounts only
what stage-1 needs:

```fstab
PARTLABEL=hyper-system /system auto ro 0 0
```

The independent system filesystem's `/etc/fstab` contains later mounts:

```fstab
PARTLABEL=hyper-data /data auto rw 0 0
```

Stage-1 never reads the formal table. The formal mount job requests it after taking
supervision. No stage-number flags are needed and the kernel parses neither table.
Sources select authorized GPT partitions with `PARTUUID=` or `PARTLABEL=`, not
Linux device paths. HypeR reads GPT and grants bounded block ranges to the
filesystem manager. The Linux I/O backend does not resolve logical volume names.

The manager identifies media by magic and launches a separate format-worker
process for each mount. `auto` selects the detected format; `fat`, `vfat` and
`erofs` require a matching format. EROFS requires `ro`. Supported options are
`defaults`, `ro`, `rw`, `auto`, `noauto` and `nofail`. The final fields must be
`0 0`; fsck and dump scheduling are not implemented. Entries run in file order.
Required bootstrap failures enter recovery; required formal failures are reported
without preventing unrelated formal services. `nofail` makes a mount optional;
`noauto` leaves it for an explicit request.

Each manager reads `etc/hyper/filesystem.json` relative to its own environment:

```json
{"worker_directories": [{"environment": "svc"}]}
```

The bootstrap manager finds static workers in its archive; the formal manager
finds formal workers in the system filesystem. Explicit `namespace` search paths
are also supported. A format needed to mount the system root must have its worker
available before switching roots. Missing workers and unsupported formats produce
runtime errors; the builder does not impose a filesystem-provider dependency graph.

```text
mount                                      list managed mounts
mount -a                                   apply the active environment's /etc/fstab
mount /data                                mount one fstab target
mount -T /data/custom.fstab -a              apply an explicit table
mount -t auto -o ro PARTLABEL=backup /data/backup
umount /data                               sync and detach an idle mount
```

Unmount rejects open files, protected mappings, directory capabilities, active
lookups and nested mounts. There is no forced or lazy unmount. Successful unmount
drains RPC, retires the mount/cache generation, stops the worker and returns its
exclusive block capability. A dead worker or unfinished block transaction is not
silently reactivated. See [filesystem services](https://hyper-proj.github.io/lib-fs-service/docs/filesystem-services.html).

## Build and customize

```sh
make
make run
make rpi5-sd
```

QEMU output defaults to `target/board/qemu/`; Pi 5 output defaults to
`target/board/rpi5-native/`. `bootstrap.cpio` contains only bootstrap files,
`system-root/` is checked formal staging, and `disk.img` contains separate system,
data and guest volumes. System files are not packaged inside the boot archive.
The removed `BOARD_SYSTEM_LAYOUT` option is rejected rather than silently ignored.
`make rpi5-bringup` is a diskless static recovery diagnostic and never launches
a formal system environment.

The default EROFS image uses uncompressed core format, 4096-byte blocks, no xattrs,
a fixed timestamp and cleared UUID. `EROFS_MKFS` selects an existing host
`mkfs.erofs`; the build does not install a global tool. FAT system partitions are
also supported when their payload contains no unsupported symbolic links.

`disk.img.system.json` records the checked system payload. This detects mismatched
builds, not runtime tampering. Ordinary `make` refreshes a changed EROFS system
partition after validating both GPT copies and capacity. It publishes a sibling
disk image atomically while preserving every byte outside that partition. An
unchanged payload leaves the disk untouched. Stop QEMU before refreshing its disk.
Writable FAT system volumes and changed partition layouts need explicit replacement.
`make run` only checks existing artifacts. `make rebuild` replaces the whole disk,
including persistent data and guest volumes. Pi replacement requires
`BOARD_IMAGE_REPLACE=--replace`.

Partition content belongs to `disk.partitions`; access and mount policy are
independent, for example:

```json
"native": {
  "layout": "split",
  "volumes": [
    {"source": "PARTLABEL=hyper-data", "read-only": false},
    {"source": "PARTLABEL=hyper-system", "read-only": true}
  ],
  "boot-fstab": "PARTLABEL=hyper-system /system auto ro 0 0\n",
  "fstab": "PARTLABEL=hyper-data /data auto rw 0 0\n"
}
```

Normal board images require a generated independent system partition. Each Native
partition consumes one of eight I/O data clients shared with guests. Client zero
is the owner-only physical block control channel. Labels are not reserved; change
their selectors with them, or use PARTUUID to distinguish equal labels. See
[board storage](https://hyper-proj.github.io/build/docs/deployment/board-storage.html) for image inputs and partition schema.

Functional checks in `tests/qemu/verify-boot-system.py` exercise root switching,
provider takeover, retirement and recovery. Parser and EROFS fixture checks run
with `make app-test APP_TEST_WORKSPACE=bootstrap/stage1 APP_TEST_PACKAGE=hyper-fs-erofs`.
VM-manager isolation checks use private disks and test-only stand-ins. These tests
establish lifecycle and filesystem behavior, not storage throughput or physical
durability.
