<!-- SPDX-FileCopyrightText: 2026 roolrz -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Stage-1 bootstrap and recovery

This directory owns programs needed in the RAM filesystem to bring up storage,
mount the system volume and provide a basic recovery shell. Ordinary tools and
the system init live in [`app/`](../../app/README.md); reusable service
implementations and the formal executable loader remain in `lib/`.

| Component | Responsibility |
| --- | --- |
| `init` | Start core services and transfer their supervision to system init |
| `userspace-loader` | Load initial and core static ELF images under the minimal kernel entry contract |
| `console-input`, `console-output` | Physical console input and output transport |
| `session` | Own the console; wait for init and supervise the selected formal/recovery shell |
| `fallback-sh` | Static recovery shell without clap, history or pipelines |
| `io-runtime` | Start the I/O VM and establish authorized Native block clients |
| `fs-backend` | Apply fstab, identify media and supervise filesystem workers |
| `fs-fat`, `fs-erofs` | Independent filesystem workers for bootstrap volumes |
| `mount`, `umount` | Inspect and repair mount state from the recovery shell |

The required filesystem workers depend on the deployment. For example, an
EROFS system volume needs `fs-erofs` before `/system` is mounted; `fs-fat` can
instead be loaded from `/system` when FAT is only used for later data mounts,
and this is the default. Stage-1 applies the RAM root's `/etc/fstab` only; formal
init applies `/system/etc/fstab` after taking supervision and switching roots.
Source placement does not force every worker into every RAM image. Missing
workers produce a runtime mount error.

Rust components own their `Cargo.toml` and `component.mk`. They belong to the
independent [`Cargo.toml`](Cargo.toml) workspace here, with its own lockfile.
Private service contracts, supervision, image-format and filesystem libraries
live in `lib/` below this workspace. They use distinct `bootstrap-*` package
identities and never link formal-system libraries from the repository's top-level
`lib/`. The resident I/O VM's configuration, image loading and backend wire
protocols belong to `io-runtime`; bootstrap does not carry the formal VM fleet
policy or guest virtio frontend state machines.
`make app-check`, `make app-test` and `make app-sdk-test` cover both workspaces.
For a targeted bootstrap test, set `APP_TEST_WORKSPACE=bootstrap/stage1` and
`APP_TEST_PACKAGE`, for example `bootstrap-fatfs`.

Only [the Native substrate](../../stdlib/README.md), Rust std, and third-party
Cargo dependencies are shared with the formal system. Services communicate via
the same capability and wire contracts; source ownership does not add IPC or a
second running service. The small C loader is built by SDK assembly using
CMake and owns its ELF mapper and stack encoder in `userspace-loader/elf/`.

Every program under `bootstrap/stage1/` must link statically, regardless of
its deployment profile or whether it is installed in `/boot` or `/system`.
The SDK linker rejects dynamic executables and shared libraries for these Cargo
packages and checks the resulting ELF for an interpreter and shared-library
dependencies. Direct `hyper-cargo build` calls must set `HYPER_LINK_MODE=static`.

`make app` builds stage-1 programs separately from the dynamic application
and library graph. `make app-boot` selects only `APP_BOOT_COMPONENTS` in that
static build tree. The console input/output workers use `no_std` and the small
Native Rust entry runtime; other Rust bootstrap services retain std. A component
selects this with the optional sixth `native-rust-app` argument `core` (default
`std`). Build and check use separate Cargo invocations and target outputs so
std features cannot leak into the `no_std` workers. The bootstrap loader is
packaged from the SDK. Composition initially installs them at the RAM root; system init
later moves that filesystem to `/boot` during `switch_root`. Ordinary applications
and shared libraries remain under `/system`. See [boot layout](../../docs/boot-layout.md) for
deployment options, fstab and the transfer of supervision from stage-1 to the
system environment.
