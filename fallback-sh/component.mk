# SPDX-FileCopyrightText: 2026 roolrz
# SPDX-License-Identifier: Apache-2.0

$(eval $(call native-rust-app,bootstrap/stage1/fallback-sh,hyper-fallback-sh,fallback-sh,svc/fallback-sh,development))
