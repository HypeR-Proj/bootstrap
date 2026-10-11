# SPDX-FileCopyrightText: 2026 roolrz
# SPDX-License-Identifier: Apache-2.0

$(eval $(call native-rust-app,bootstrap/stage1/io-runtime,hyper-io-runtime,io-runtime,svc/io-runtime,io))
