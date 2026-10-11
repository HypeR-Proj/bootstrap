// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

//! Bounded recovery input editing; no history storage or navigation.

use crate::command::MAX_LINE_BYTES;

pub enum Action {
    None,
    Echo(u8),
    Redraw,
    ClearScreen,
    Submit(Vec<u8>),
    TooLong,
    Cancel,
    Exit,
}

#[derive(Default)]
enum Escape {
    #[default]
    None,
    Start,
    Sequence,
}

#[derive(Default)]
pub struct Editor {
    line: Vec<u8>,
    escape: Escape,
    overflow: bool,
    carriage_return: bool,
}

impl Editor {
    pub fn line(&self) -> &[u8] {
        &self.line
    }

    pub fn push(&mut self, byte: u8) -> Action {
        if byte == b'\n' && self.carriage_return {
            self.carriage_return = false;
            return Action::None;
        }
        self.carriage_return = byte == b'\r';
        match byte {
            b'\r' | b'\n' => return self.submit(),
            3 => {
                self.reset();
                return Action::Cancel;
            }
            21 => {
                self.reset();
                return Action::Redraw;
            }
            12 => {
                self.escape = Escape::None;
                return Action::ClearScreen;
            }
            27 => {
                self.escape = Escape::Start;
                return Action::None;
            }
            _ => (),
        }
        match std::mem::take(&mut self.escape) {
            Escape::Start => {
                if matches!(byte, b'[' | b'O') {
                    self.escape = Escape::Sequence;
                }
                return Action::None;
            }
            Escape::Sequence => {
                if (0x40..=0x7e).contains(&byte) {
                    return Action::None;
                }
                if (0x20..=0x3f).contains(&byte) {
                    self.escape = Escape::Sequence;
                }
                return Action::None;
            }
            Escape::None => (),
        }
        match byte {
            4 if self.line.is_empty() => Action::Exit,
            8 | 127 if !self.overflow => {
                if self.line.is_empty() {
                    return Action::None;
                }
                // Remove the complete final UTF-8 character, not just its last byte.
                while self.line.last().is_some_and(|b| b & 0xc0 == 0x80) {
                    self.line.pop();
                }
                self.line.pop();
                Action::Redraw
            }
            byte if byte == b'\t' || (byte >= 0x20 && byte != 127) => {
                if self.overflow {
                    return Action::None;
                }
                if self.line.len() == MAX_LINE_BYTES {
                    self.overflow = true;
                    return Action::None;
                }
                self.line.push(byte);
                Action::Echo(byte)
            }
            _ => Action::None,
        }
    }

    fn submit(&mut self) -> Action {
        let overflow = self.overflow;
        let line = std::mem::take(&mut self.line);
        self.reset();
        if overflow {
            return Action::TooLong;
        }
        Action::Submit(line)
    }

    fn reset(&mut self) {
        self.line.clear();
        self.overflow = false;
        self.escape = Escape::None;
    }
}

#[cfg(test)]
#[path = "../tests/editor.rs"]
mod tests;
