# SPDX-FileCopyrightText: 2026 roolrz
# SPDX-License-Identifier: Apache-2.0

$(eval $(call native-rust-app,bootstrap/stage1/fs-erofs,hyper-fs-erofs,fs-erofs,svc/fs-erofs,system development))
