<!--
SPDX-FileCopyrightText: 2026 roolrz
SPDX-License-Identifier: Apache-2.0
-->

# Stage-1 executable loader

This component builds `userspace-loader-hyper-aarch64` and
`userspace-loader-hyper-riscv64`. Kernel init opens
`/lib/userspace-loader-hyper-<arch>` to start `/init`; deployment provides the
usual `lib -> lib64` alias. Subsequent process builders receive an explicit
loader File capability. The kernel does not select a loader for their children.

The stage-1 loader starts static core services and rejects `PT_INTERP` before
mapping any application segment. It has no allocator, dependencies, TLS, symbol
lookup, relocation, constructors or service process. It owns its internal [ELF loader primitives](elf/README.md).
The [formal-environment loader](../../../lib/dynamic-loader/README.md) loads
both static and dynamic applications outside stage-1.

## Kernel boundary

The accepted bootstrap image is a Native little-endian ELF64 `ET_DYN`, matching
the host architecture, with at most eight nonoverlapping `PT_LOAD` segments and
a 1 MiB image span. Their page-rounded mappings cover one contiguous range.
Kernel admission rejects interpreters, TLS and executable stacks; it ignores
permitted dynamic/RELRO metadata rather than interpreting or relocating it.
The stage-1 link script additionally requires no runtime relocations and no
writable global state.

The first argument register (`x0` or `a0`) holds a child-owned ByteChannel handle.
SP identifies the top of an empty guarded 128 KiB stack. One queued message
contains the 96-byte `loader_startup` header, tagged startup handle records,
and bounded opaque launch data. It supplies an ordinary executable File with `READ|EXECUTE|PROTECT`,
root/stack VMAR handles, bootstrap mapping bounds, a reply flag and the child's
ExecutionContext, ObjectFactory and SystemInfo capabilities. The initial channel
read requires only the explicit channel handle. The stage-1 loader passes context
handles in the entry vector; it uses explicit receivers throughout and retains no
writable invocation globals. A loading failure uses the supplied ExecutionContext
to exit; a malformed header without a trustworthy context terminates by trap. The loader
acquires content protection before obtaining the size or reading any ELF bytes.
There is no ambient runtime-directory capability in this record.

The SDK interprets argv/environment data and constructs the application stack.
The kernel owns executable provenance, W^X, COW, accounting, cache/TLB coherence
and process retirement. A process builder retains the target File identity without freezing its
contents. Only the restricted bootstrap loader uses a kernel-internal temporary
stable copy during preparation. The startup reply confirms userspace loading and
runtime setup; a later failure is a committed child failure, not a retryable
builder operation.

## Static handoff and memory

The initial assembly reserves the upper 64 KiB of the stack for the message,
startup handle table and LP64 entry vector; C frames use the lower half.
The ELF mapper validates ranges before installing private COW mappings of the
protected File. It rejects overlap, W+X, TLS, malformed headers and executable
stacks. Main images occupy the `[0x00400000, 0x10000000)` window on both supported
architectures; static PIE is rebased, while `ET_EXEC` retains linked addresses.

The constructed vector contains `argc`, `argv`, `envp`, ELF auxv and SDK-private
startup metadata. `AT_BASE` is zero so the static CRT performs its own relative
relocation. The loader closes its File and parser protection token, then enters
the application CRT on the initial thread, without another loader or IPC
rendezvous. Each mapping retains content protection until unmap or owner-process
teardown; it does not retain a historical file snapshot. Clean mapped pages can be reclaimed and faulted
back from the protected file, while modified private COW pages remain owned by
the process. Once the owner's threads stop, teardown detaches protected sources
without allocating. An externally retained VMAR does not extend protection or
permit file refills after that teardown.

After copying startup data and switching to its final guarded stack, the app's
runtime unmaps this loader and the temporary stack, acknowledges startup if a
parent is waiting, and enters application code. No live pointers may remain in
the retired bootstrap mappings. Final stack/heap placement remains runtime
policy; the loader only forwards `PT_GNU_STACK` and actual bootstrap bounds.

## Verification

`sh sdk/toolchain/scripts/check-userspace-loader.sh` exercises the production
mapper, message decoder, stack construction and both entry policies for AArch64
and RV64 using syscall substitutes. Tests cover protection failure before any read, short file reads, protection
release on parser failure, rejected interpreters, static
and dynamic retirement metadata, maximal startup packets and relative
relocations. Native images and QEMU acceptance additionally verify the mapped
entry and final-stack transitions; host tests do not emulate page tables.
