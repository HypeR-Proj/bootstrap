/* SPDX-FileCopyrightText: 2026 roolrz
 * SPDX-License-Identifier: Apache-2.0
 */
ENTRY(_start)
PHDRS {
    headers PT_LOAD FILEHDR PHDRS FLAGS(4);
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
    stack PT_GNU_STACK FLAGS(6);
}
SECTIONS {
    . = SIZEOF_HEADERS;
    .identity : { BYTE(0) } :headers
    . = ALIGN(4096);
    .text : { KEEP(*(.text._start)) *(.text .text.*) } :text
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.* .srodata .srodata.*) } :rodata
    . = ALIGN(4096);
    .data : { *(.data .data.* .sdata .sdata.*) *(.got .got.*) } :data
    .bss (NOLOAD) : { *(.bss .bss.* .sbss .sbss.*) *(COMMON) } :data
    .tdata : { *(.tdata .tdata.*) } :data
    .tbss (NOLOAD) : { *(.tbss .tbss.*) } :data
    .rela : { *(.rela.* .rel.* .relr.*) } :rodata
    /DISCARD/ : { *(.dynamic .dynsym .dynstr .hash .gnu.hash) *(.comment .note .note.* .eh_frame .eh_frame_hdr) }
}
ASSERT(SIZEOF(.rela) == 0, "userspace-loader must not require relocation")
ASSERT(SIZEOF(.data) + SIZEOF(.bss) == 0, "userspace-loader must not carry writable global state")
ASSERT(SIZEOF(.tdata) + SIZEOF(.tbss) == 0, "userspace-loader must not require TLS")
