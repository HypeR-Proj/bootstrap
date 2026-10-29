<!--
SPDX-FileCopyrightText: 2026 roolrz
SPDX-License-Identifier: Apache-2.0
-->

# Boot, system and data deployment

The initial RAM archive contains only static bootstrap and recovery programs.
Stage-1 mounts the independent system volume temporarily at `/system`, then
switches that filesystem to `/` **before** loading system init. The default system
volume is read-only EROFS; writable persistent files live on the FAT volume at
`/data`. Formal programs and shared libraries are not embedded in the bootstrap
archive. Plain `make`, `make rebuild` and `make run` use this deployment.

The kernel mounts the supplied RAM archive as `/` and starts `/init` through
`/lib/userspace-loader-hyper-<arch>`. Those are its only bootstrap path
conventions. The temporary `/system` path, `/boot`, stage-1, and the service handoff
are userspace policy. The system environment has no stage number;
`component-manager` is its root supervisor. The Pi firmware's FAT boot partition is separate from the
Native `/boot` mount.

## Files and dependencies

[`bootstrap/stage1`](https://github.com/HypeR-Proj/bootstrap/blob/main/README.md) owns static bootstrap
programs. Formal commands live in `app/` and background providers in `svc/`, and their reusable
implementations live in `lib/`. Bootstrap does not link these formal libraries.
It may use the Native kernel-facility substrate, static Rust std and third-party
Cargo crates. Supervisor, format and I/O implementation modules belong to their
own bootstrap executable packages; there is no `stage1/lib` package graph.

The initial archive directly contains `/init`, `/svc`, `/bin`, `/etc`, `/vm`,
`/lib64` and `/lib -> lib64`. After switching roots, the layout is:

```text
/bin/                              formal shell and Native commands
/svc/                              component-manager and formal services
/etc/components/*.json             reusable component declarations
/etc/hyper/components.json         instance-tree root and configuration directory
/etc/hyper/startup.json            finite provider-adoption policy
/etc/fstab                         later mounts requested by /mounts
/lib64/ld-hyper-<arch>.so           formal executable loader and dynamic linker
/lib64/<arch>-hyper-hyper/          shared libraries
/lib -> lib64                      library-directory alias
/boot/                             original archive until bootstrap retirement
/tmp/                              separate writable RAM filesystem
/data/                             persistent FAT volume mounted in the formal environment
```

The system image includes empty `/boot`, `/tmp` and `/data` mountpoint directories,
so preparing the transition does not require writes to EROFS. Stage-1 switches the
mounted system filesystem to the namespace root. The old archive becomes `/boot`;
the temporary `/system` is not a second path to the formal root. After accepting
supervision, component-manager commits the root transition. System-startup mounts
a fresh RAM filesystem at `/tmp` before preparing replacement providers. After provider adoption,
bootstrap-retire stops predecessor processes and detaches `/boot`. Its empty
mountpoint directory remains. The last kernel owner of an archive-backed file or
page releases the corresponding physical backing.

Bootstrap launchers retain fixed `process.environment-directory` capabilities
for their own files as the archive moves to `/boot`. Their separate namespace
root capability follows the root switch. A fixed file or subdirectory capability
keeps its existing identity and confinement; root switching does not reopen files
or change their contents. The filesystem itself and its mounted descendants move
as one namespace transaction. Ordinary kernel RAMFS and namespace mechanisms do
not embed any of these deployment paths.

Formal init is opened only after the root switch. It and every formal child use
the independent system filesystem for executable and library lookup. Core services
use the static bootstrap loader. Formal programs, including static executables,
use the delegated `/lib64/ld-hyper-<arch>.so`; dynamic linking also uses the
`/lib64/<arch>-hyper-hyper` Directory capability. A dynamic ELF's canonical
`PT_INTERP` is `/lib64/ld-hyper-<arch>.so`. `ldd` inspects that path literally:

```sh
ldd /bin/vmm
ldd --tree /svc/vm-runtime
```

The default bootstrap selection contains init, console-raw, fallback-sh,
io-runtime, fs-backend, fs-erofs, fs-fat, mount and umount. `APP_BOOT_COMPONENTS`
selects staged component names. Each bootstrap executable must be static; the
SDK driver and final ELF checks reject an interpreter or shared-library dependency.
The default keeps the static FAT worker available for recovery, while the formal
image also contains its independent FAT worker. Removing `fs-fat` from the boot
selection does not remove the formal `fs-fat-system` component.

Build artifacts retain the selected debug information policy. Packaging strips
only debug sections from temporary copies. The formal executable and transitive
DSO closure is checked for architecture, SONAME and symbol compatibility before
publication.

## Supervision handoff

Stage-1 validates the selected profile and its service manifest. The default
profile starts raw console, I/O VM storage and filesystem services, then waits
for mounts in the archive's `/etc/fstab`. Another profile can select a Native
storage provider or recovery without I/O. The I/O VM, when selected, starts
independently of the business VM manager; keeping Native storage alive does not
require a business fleet or provisioning connection. Bootstrap contains no
terminal service or line discipline. The default profile launches its byte-stream
fallback shell only on failure; a recovery-only profile may launch it directly.

Once `/system` is an independent mount, stage-1 prepares a reversible root
transition. It starts `/svc/component-manager` in a separate candidate task
group. The manager reads the component tree and prepares system-startup, which
validates the offered capability inventory and migration policy. One capability-channel rendezvous transfers core Process
supervisors and their management inventory. That transfer is the supervision
commit point. Before commit, failure retains stage-1 ownership and rolls the
namespace back for recovery. After commit, the formal environment owns failure
handling and retirement timing; stage-1 does not restart its fallback shell.

Component-manager forwards domain resources to system-startup, which replaces
the offered bootstrap console, storage and filesystem providers with admitted
formal processes. The selected storage implementation owns its migration
protocol. With the default I/O VM provider, the running VM and mounted filesystem
identities survive adoption: the VM does not reboot and volumes are not remounted.
New providers start before
predecessors retire. A failed adoption retains still-needed predecessor resources.
The finite coordinator applies the formal fstab through a mount job, starts
formal services and configures the VM fleet. An independent retirement job
runs after confirmed provider adoption; successful exit publishes the
bootstrap-retired progress fact. Data mount failures are handled in the formal environment and do not
return to stage-1. See the [handoff contract](https://hyper-proj.github.io/lib-bootstrap-handoff/docs/handoff.html) for transfer
records, ownership and retirement conditions.

## Configuration and operation

See [boot profiles and mount policy](boot-policy.md) for profile
selection, fstab entries, recovery behavior and image customization.
