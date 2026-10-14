<!-- SPDX-FileCopyrightText: 2026 roolrz -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# EROFS media reader

`hyper-erofs` reads immutable EROFS images through a bounded random-read device.
The Native `fs-erofs` process adapts that interface to its exclusive block-volume
capability. Generic filesystem RPC, mount publication, and namespace symlink
resolution remain outside the format reader.

The supported format is uncompressed EROFS with 512, 1024, 2048, or 4096-byte
blocks, compact and extended inodes, flat contiguous data and inline tails,
regular files, directories, and symbolic links. A present superblock CRC32C is
validated. Unsupported incompatible features and inode layouts, including
compression, chunked files, external devices, and extended attributes, return
an explicit error. Device nodes, sockets, and FIFOs are also unsupported.

The parser uses safe Rust and checks disk offsets against the actual device
length. It caches one 4 KiB metadata block and one resolved path, without loading
whole files or images. Directory scans are limited to 16 MiB per directory;
paths and symlink targets to 4096 bytes; filenames to 255 bytes. HypeR's namespace
requires UTF-8 names. Symlink target bytes are returned without interpretation.
The device must remain immutable for the lifetime of a mounted reader.

`Volume::read_directory(path, cursor, output)` writes a directory batch of at
most 4 KiB using the common `hyper-filesystem` protocol. Each batch carries
names, complete supported attributes, per-entry continuation checkpoints and
an EOF flag. A zero cursor resolves the path; subsequent requests restore the
directory inode, block offset and entry position, without rescanning earlier
blocks. The reader validates the current block and its ordering against the
preceding block. Separate readers can interleave requests without shared scan
state. The indexed `entry(path, index)` interface has been removed.

Checkpoints are private to the kernel directory stream, bound to its mounted
directory; callers must not accept physical checkpoints from applications.
The parser checks cursor format, path fingerprint, inode geometry and directory
bounds, while the kernel owns authority and mount lifetime. The worker retains
no directory snapshot or per-reader allocation. Kernel directory/name/attribute
caches are optional and reclaimable under the same memory pressure as file
data pages; after eviction, the retained stream position still permits a
bounded read from the worker. The immutable EROFS device requires no media
mutation invalidation while mounted.

The implementation follows the published [EROFS core disk format](https://erofs.docs.kernel.org/en/latest/ondisk/core_ondisk.html).
It does not incorporate Linux filesystem implementation code.

Run parser tests with `cargo test --manifest-path bootstrap/stage1/Cargo.toml -p hyper-fs-erofs`.
The host `inspect` example lists a directory or emits file/link bytes from an image:

```sh
cargo run --manifest-path bootstrap/stage1/Cargo.toml -p hyper-fs-erofs --example inspect -- IMAGE [PATH]
```
