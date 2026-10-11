# SPDX-FileCopyrightText: 2026 roolrz
# SPDX-License-Identifier: Apache-2.0

cmake_minimum_required(VERSION 3.25)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
project(HypeRUserspaceLoader VERSION 0.0.0 LANGUAGES C ASM)

if(NOT CMAKE_C_COMPILER_ID MATCHES "Clang")
    message(FATAL_ERROR "HypeR userspace-loader requires Clang")
endif()
if(NOT HYPER_ARCH MATCHES "^(aarch64|riscv64)$")
    message(FATAL_ERROR "HypeR userspace-loader requires a supported Native architecture")
endif()
if(NOT EXISTS "${HYPER_SYSROOT}/include/hyper/native.h")
    message(FATAL_ERROR "HYPER_SYSROOT must name a complete HypeR sysroot")
endif()

add_executable(hyper_userspace_loader
    src/arch/${HYPER_ARCH}/start.S
    src/start.c
)
include(${CMAKE_CURRENT_SOURCE_DIR}/../../../lib/elf-loader/CMakeLists.txt)
hyper_add_elf_loader(hyper_userspace_loader)
target_compile_features(hyper_userspace_loader PRIVATE c_std_17)
target_compile_options(hyper_userspace_loader PRIVATE
    $<$<COMPILE_LANGUAGE:C>:-ffreestanding;-fno-builtin;-fPIE;-fno-stack-protector;-fvisibility=hidden;-Wall;-Wextra;-Werror>
)
if(HYPER_ARCH STREQUAL "riscv64")
    # LLVM 18 places small constant pools in writable .sdata. Keep constants
    # read-only so the bootstrap can share its image without writable state.
    target_compile_options(hyper_userspace_loader PRIVATE
        $<$<COMPILE_LANGUAGE:C>:-msmall-data-limit=0>
    )
endif()
target_include_directories(hyper_userspace_loader PRIVATE
    ${HYPER_SYSROOT}/include
)
target_link_options(hyper_userspace_loader PRIVATE
    --ld-path=${HYPER_LD}
    -nostdlib
    -Wl,-pie,--no-dynamic-linker,--no-undefined,--fatal-warnings
    -Wl,-Bsymbolic,-z,text,--entry=_start
    -Wl,--build-id=none,--gc-sections,-z,noexecstack,-z,max-page-size=4096
    -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/userspace-loader.ld
    ${HYPER_SYSROOT}/lib/libhyper.a
)
set_target_properties(hyper_userspace_loader PROPERTIES
    OUTPUT_NAME userspace-loader-hyper-${HYPER_ARCH}
    SUFFIX ""
)

install(TARGETS hyper_userspace_loader RUNTIME DESTINATION lib64)
