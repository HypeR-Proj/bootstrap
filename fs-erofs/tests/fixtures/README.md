<!-- SPDX-FileCopyrightText: 2026 roolrz -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

`core.erofs` is test data produced by upstream erofs-utils v1.8.10:

```sh
printf 'EROFS reference fixture\n' > SOURCE/hello
ln -s hello SOURCE/link
mkfs.erofs -b4096 -x-1 -T0 -Uclear --all-root core.erofs SOURCE
```

Its stored superblock CRC32C provides an independent reference for the reader's
checksum implementation. The fixture contains no upstream implementation code.
