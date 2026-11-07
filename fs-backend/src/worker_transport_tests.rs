// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bootstrap never delegates asynchronous persistence, even to a capable driver.
use super::*;

#[derive(Default)]
struct Backend {
    readonly: bool,
    callbacks: usize,
}

impl Backend {
    fn unexpected<T>(&mut self) -> Result<T, Error> {
        self.callbacks += 1;
        Err(Error::Corrupt)
    }
}

impl Volume for Backend {
    fn capabilities(&self) -> protocol::Capabilities {
        protocol::Capabilities::from_bits(
            protocol::Capabilities::READ_EXTENTS.bits()
                | protocol::Capabilities::ASYNC_WRITEBACK.bits(),
        )
    }
    fn read_only(&self) -> bool {
        self.readonly
    }
    fn stat(&mut self, _: &str) -> Result<Entry, Error> {
        self.callbacks += 1;
        Ok(Entry::empty())
    }
    fn read_directory(
        &mut self,
        _: &str,
        _: protocol::DirectoryCursor,
        _: &mut [u8],
    ) -> Result<usize, Error> {
        self.unexpected()
    }
    fn read(&mut self, _: &str, _: u64, _: &mut [u8]) -> Result<usize, Error> {
        self.unexpected()
    }
    fn write(&mut self, _: &str, _: u64, _: &[u8]) -> Result<usize, Error> {
        self.unexpected()
    }
    fn create(&mut self, _: &str, _: bool) -> Result<(), Error> {
        self.unexpected()
    }
    fn remove(&mut self, _: &str) -> Result<(), Error> {
        self.unexpected()
    }
    fn rename(&mut self, _: &str, _: &str) -> Result<(), Error> {
        self.unexpected()
    }
    fn resize(&mut self, _: &str, _: u64) -> Result<(), Error> {
        self.unexpected()
    }
    fn sync(&mut self) -> Result<(), Error> {
        self.unexpected()
    }
    fn set_times(
        &mut self,
        _: &str,
        _: Option<hyper_filesystem::Timestamp>,
        _: Option<hyper_filesystem::Timestamp>,
    ) -> Result<(), Error> {
        self.unexpected()
    }
}

fn invoke(backend: &mut Backend, operation: Operation) -> Result<Result<u64, Error>, String> {
    let request = Request {
        sequence: 1,
        operation,
        offset: 0,
        length: 0,
        path: "",
        target: "",
        accessed: None,
        modified: None,
        window_token: 0,
        cursor: protocol::DirectoryCursor::START,
    };
    let mut failure = None;
    let mut reply = [0; protocol::RESPONSE_BYTES];
    dispatch(backend, &mut failure, request, &mut [], &[], &mut reply)?;
    assert_eq!(failure, None);
    Ok(Response::decode(&reply, 1, operation)
        .ok_or("invalid response")?
        .result)
}

#[test]
fn capability_and_delegation_remain_synchronous() -> Result<(), String> {
    let mut backend = Backend::default();
    assert_eq!(
        invoke(&mut backend, Operation::Capabilities)?,
        Ok(protocol::Capabilities::READ_EXTENTS.bits())
    );
    assert_eq!(invoke(&mut backend, Operation::TakeWriteback)?, Ok(0));
    assert_eq!(backend.callbacks, 0);
    Ok(())
}

#[test]
fn async_rejection_has_no_mutation_sync_or_sticky_failure() -> Result<(), String> {
    for readonly in [false, true] {
        let mut backend = Backend {
            readonly,
            callbacks: 0,
        };
        assert_eq!(
            invoke(&mut backend, Operation::SubmitWriteback)?,
            Err(if readonly {
                Error::ReadOnly
            } else {
                Error::Unsupported
            })
        );
        assert_eq!(
            invoke(&mut backend, Operation::ReapWriteback)?,
            Err(Error::Unsupported)
        );
        assert_eq!(backend.callbacks, 0);
        assert_eq!(invoke(&mut backend, Operation::Stat)?, Ok(0));
        assert_eq!(backend.callbacks, 1);
    }
    Ok(())
}
