// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Discovery uses the appliance's bounded read window; Native initiators use
//! their own queue grant. Both occupy disjoint slots in one guest aperture.

const DISCOVERY_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Geometry {
    base: u64,
    bytes: u64,
    end: u64,
}

impl Geometry {
    pub(super) fn new(shared_base: u64, client: u32, native_bytes: u64) -> Option<Self> {
        if native_bytes < DISCOVERY_BYTES
            || !native_bytes.is_power_of_two()
            || !shared_base.is_multiple_of(native_bytes)
        {
            return None;
        }
        let base = shared_base.checked_add(u64::from(client).checked_mul(native_bytes)?)?;
        let bytes = if client == 0 {
            DISCOVERY_BYTES
        } else {
            native_bytes
        };
        let end = base.checked_add(bytes)?;
        Some(Self { base, bytes, end })
    }

    pub(super) fn base(self) -> u64 {
        self.base
    }

    pub(super) fn bytes(self) -> u64 {
        self.bytes
    }

    pub(super) fn end(self) -> u64 {
        self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn larger_native_grants_preserve_the_published_discovery_window() -> Result<(), &'static str> {
        let base = 0x4400_0000;
        let native = 2 * 1024 * 1024;
        let discovery = Geometry::new(base, 0, native).ok_or("discovery geometry rejected")?;
        assert_eq!(discovery.base(), base);
        // The published block-reader backend rejects larger discovery windows.
        assert_eq!(discovery.bytes(), 1024 * 1024);
        assert_eq!(discovery.end(), base + 1024 * 1024);
        let first = Geometry::new(base, 1, native).ok_or("first Native geometry rejected")?;
        let second = Geometry::new(base, 2, native).ok_or("second Native geometry rejected")?;
        assert_eq!(first.bytes(), native);
        assert_eq!(first.base(), base + native);
        assert_eq!(second.base(), first.end());
        assert!(discovery.end() <= first.base());
        assert!(first.base().is_multiple_of(64 * 1024));
        assert!(first.bytes().is_multiple_of(64 * 1024));
        Ok(())
    }

    #[test]
    fn invalid_or_wrapping_geometry_is_rejected_before_allocation() {
        let native = 2 * 1024 * 1024;
        assert!(Geometry::new(1, 0, native).is_none());
        assert!(Geometry::new(0, 1, 1024 * 1024 + 32768).is_none());
        assert!(Geometry::new(0, 0, 512 * 1024).is_none());
        let last_aligned = !(native - 1);
        assert!(Geometry::new(last_aligned, 1, native).is_none());
        assert!(Geometry::new(last_aligned, 0, native).is_some());
        assert!(Geometry::new(last_aligned, 2, native).is_none());
        assert!(Geometry::new(1 << 63, u32::MAX, 1 << 63).is_none());
        assert!(Geometry::new(0, 1, 1 << 63).is_none());
    }
}
