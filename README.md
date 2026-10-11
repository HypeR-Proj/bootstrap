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
instead be loaded from `/system` when FAT is only used for later data mounts.
Source placement does not force every worker into every RAM image. Missing
workers produce a runtime mount error.

Rust components own their `Cargo.toml` and `component.mk`. These packages belong
to the existing [`app/Cargo.toml`](../../app/Cargo.toml) delivery workspace and
share its SDK dependencies and lockfile. `make app-check` and `make app-test`
include them; no second dependency workspace is needed. The small C bootstrap
loader is built by SDK assembly using CMake and shares bounded ELF mapping and
stack construction code with the formal loader in `lib/elf-loader`.

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
