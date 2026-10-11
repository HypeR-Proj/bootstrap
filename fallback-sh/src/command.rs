// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bounded shell words using the standard POSIX shlex syntax.

pub const MAX_LINE_BYTES: usize = 512;
pub const MAX_ARGUMENTS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    LineTooLong,
    TooManyArguments,
    InvalidSyntax,
}

pub struct CommandLine(Vec<String>);

impl CommandLine {
    pub fn parse(input: &[u8]) -> Result<Self, ParseError> {
        if input.len() > MAX_LINE_BYTES {
            return Err(ParseError::LineTooLong);
        }
        let text = std::str::from_utf8(input).map_err(|_| ParseError::InvalidSyntax)?;
        if text.contains('\0') {
            return Err(ParseError::InvalidSyntax);
        }
        let words = shlex::split(text).ok_or(ParseError::InvalidSyntax)?;
        if words.len() > MAX_ARGUMENTS {
            return Err(ParseError::TooManyArguments);
        }
        Ok(Self(words))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn argument(&self, index: usize) -> Option<&str> {
        self.0.get(index).map(String::as_str)
    }
    pub fn arguments(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

#[cfg(test)]
#[path = "../tests/command.rs"]
mod tests;
