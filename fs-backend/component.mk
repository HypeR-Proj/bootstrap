# SPDX-FileCopyrightText: 2026 roolrz
# SPDX-License-Identifier: Apache-2.0

$(eval $(call native-rust-app,bootstrap/stage1/fs-backend,hyper-fs-backend,fs-backend,svc/fs-backend,system development))
