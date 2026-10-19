/* SPDX-FileCopyrightText: 2026 roolrz
 * SPDX-License-Identifier: Apache-2.0
 */
/* Stage-1 only: static core images, without interpreter or library lookup. */
#include "launch.h"

extern _Noreturn void __hyper_userspace_enter(uintptr_t stack, uintptr_t entry);

_Noreturn void __hyper_userspace_start(hyper_native_handle_t channel, uintptr_t stack_top)
{
	ElfLaunch launch;
	hyper_native_status_t status = elf_launch_receive(&launch, channel, stack_top);
	if (status)
		elf_launch_fail(channel, launch.boot ? launch.boot->execution_context : 0, status);
	ElfImage main;
	status = elf_image_load(&main, launch.boot->root_vmar, launch.boot->executable, 0x400000,
				0x10000000, NULL);
	(void)hyper_handle_close(launch.boot->executable);
	if (status)
		elf_launch_fail(channel, launch.boot->execution_context, status);
	uintptr_t stack = elf_launch_stack(&launch, &main, channel, 0, 1);
	__hyper_userspace_enter(stack, main.entry);
}
