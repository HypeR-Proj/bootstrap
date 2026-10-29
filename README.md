<!-- SPDX-FileCopyrightText: 2026 roolrz -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Stage-1 bootstrap and recovery

Build and deployment options are declared in this directory's [Kconfig](Kconfig).
Use the repository's `make menuconfig` and `.config` to select programs; see
[product configuration](https://hyper-proj.github.io/manifest/docs/configuration.html) for dependencies and defconfig workflows.

This directory owns programs needed in the RAM filesystem to bring up storage,
mount the system volume and provide a basic recovery shell. Ordinary tools and
the system init live in [`svc/`](https://github.com/HypeR-Proj/build/blob/main/svc/README.md); reusable service
implementations and the formal executable loader remain in `lib/`.

| Component | Responsibility |
| --- | --- |
| `init` | Select a boot profile, start its services and transfer their supervision to system init |
| `userspace-loader` | Load initial and core static ELF images under the minimal kernel entry contract |
| `console-raw` | Byte-only physical Console transport and independent service log mirroring |
| `fallback-sh` | Static recovery shell without clap, history or pipelines |
| `io-runtime` | Start the I/O VM and establish authorized Native block clients |
| `fs-backend` | Apply fstab, identify media and supervise filesystem workers |
| `fs-fat`, `fs-erofs` | Independent filesystem workers for bootstrap volumes |
| `mount`, `umount` | Inspect and repair mount state from the recovery shell |

The required filesystem workers depend on the deployment. For example, an
EROFS system volume needs `fs-erofs` before `/system` is mounted; `fs-fat` can
be omitted from the boot selection when FAT is only needed after handoff.
The formal environment deploys its own FAT worker. The default image keeps both
bootstrap workers in boot RAM and independent formal workers on the system root. Stage-1 applies the RAM root's
`/etc/fstab` only; formal init applies `/etc/fstab` after taking supervision; stage-1 switches roots before launching it. The boot and formal worker search policies each use their
own environment directory.
Source placement does not force every worker into every RAM image. Missing
workers declared as profile prerequisites are rejected during packaging;
filesystem formats selected only by runtime media probing still report a mount
error when their worker is unavailable.

`/etc/hyper/boot-profiles/default.json` is required. Stage-1 interprets
`hyper.boot-profile=NAME` from the arguments left by the kernel to select
another packaged profile. Stage-1 closes the received boot-argument handle after
reading it, and passes unrecognized arguments to system init in a new read-only
VMO. Each profile names its own service manifest and
explicit storage provider. The built-in `io-vm` source starts the I/O VM; a
`service` source can supply storage without VM authority, and `none` omits it.
The handoff is independent of that choice: stage-1 establishes the independent
root, while formal policy admits the selected provider's successor before
ownership moves. Stage-1 does not interpret I/O VM migration payloads or select
formal executables on the provider's behalf. See
[boot profiles](docs/boot-policy.md#boot-profiles) for the schema and build
validation boundary.

Rust components own their `Cargo.toml` and `component.mk`. They belong to the
independent [`Cargo.toml`](Cargo.toml) workspace here, with its own lockfile.
There is no `stage1/lib` tree or private bootstrap library package graph.
Supervisor and format engines live in their owning executable packages.
`contracts/` supplies source modules for bootstrap wire formats and capability
roles; these are compiled as part of the consuming programs. The block parser
and guest-image loader belong to `io-runtime/src/partition.rs` and `io-runtime/src/image/`.
The filesystem request loop is owned by `fs-backend/src/worker_transport.rs` and included
by the format workers. Bootstrap never links formal-system libraries from the
repository's top-level `lib/`. Its I/O runtime does not carry formal VM fleet
policy or guest virtio frontend state machines.
`make app-check`, `make app-test` and `make app-sdk-test` cover both workspaces.
For a targeted bootstrap test, set `APP_TEST_WORKSPACE=bootstrap/stage1` and
`APP_TEST_PACKAGE`, for example `hyper-fs-fat`.

Only [the Native substrate](https://github.com/HypeR-Proj/build/blob/main/stdlib/README.md), Rust std, and third-party
Cargo dependencies are shared with the formal system. Services communicate via
explicit capability and wire contracts. During takeover, replacement processes
run alongside their predecessors until system init completes the ownership
transfer and retires the old processes. The small C loader is built by SDK assembly using
CMake and owns its ELF mapper and stack encoder in `userspace-loader/elf/`.

Every program under `bootstrap/stage1/` must link statically, regardless of
its deployment profile. Formal providers are separate components from `svc/`.
The SDK linker rejects dynamic executables and shared libraries for these Cargo
packages and checks the resulting ELF for an interpreter and shared-library
dependencies. Direct `hyper-cargo build` calls must set `HYPER_LINK_MODE=static`.

`make app` builds stage-1 programs separately from the dynamic application
and library graph. `make app-boot` selects only `APP_BOOT_COMPONENTS` in that
static build tree. The raw Console service uses statically linked std for bounded
admission workers, without a terminal library or line discipline. Components can
select a freestanding runtime with the optional sixth `native-rust-app` argument
`core` (default `std`); separate Cargo outputs prevent feature unification across
those runtime graphs. The bootstrap loader is
packaged from the SDK. Composition initially installs bootstrap at the RAM root.
Stage-1 mounts the independent system volume, switches it to `/` while retaining
the archive at `/boot`, and only then starts formal init. Provider adoption and
retirement let formal init stop predecessor processes and detach that archive.
Ordinary applications and shared libraries live on the independent system root.
See [boot layout](docs/boot-layout.md) for deployment policy, fstab and the
transfer of supervision.

The init supervisor implementation is local to `init/`. FAT and EROFS engines
are local to their respective worker programs. These single-consumer modules do
not form independently linked bootstrap library packages. The
[handoff contract](https://hyper-proj.github.io/lib-bootstrap-handoff/docs/handoff.html) separates supervision
adoption from retirement, which is decided and executed by system init.

Bootstrap service diagnostics use a boot timestamp, phase and service header,
for example `[    1.234567][HypeR bootstrap stage 1][io-runtime] IO VM: ...`.
Timestamps use HypeR's monotonic boot clock and the kernel's seconds/microseconds
format, captured before log buffering. An unavailable clock is shown as
`[    ?.??????]` without suppressing the diagnostic.
The formal root runs as `/svc/component-manager` and logs under that name.
It prepares `system-startup` to validate the domain inventory before accepting
ownership. Stage1's candidate group includes both processes, so rejection stops
both before rolling back. Component routes, journal policy and the finite formal
startup sequence are not interpreted by stage1. See
[component management](https://hyper-proj.github.io/component-manager/docs/components.html).

## Workspace tests

Use the [HypeR workspace](https://github.com/HypeR-Proj/HypeR) to resolve dependencies:

```sh
python3 project.py test bootstrap
```

Paths in the documentation are relative to the repo-managed workspace root.
Edit this repository and prepare the workspace again before building.
