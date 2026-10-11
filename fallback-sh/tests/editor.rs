// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn type_bytes(editor: &mut Editor, bytes: &[u8]) {
    for byte in bytes {
        editor.push(*byte);
    }
}

#[test]
fn overlong_input_is_rejected_instead_of_executing_its_prefix() {
    let mut editor = Editor::default();
    type_bytes(&mut editor, &vec![b'x'; MAX_LINE_BYTES + 1]);
    assert!(matches!(editor.push(b'\n'), Action::TooLong));
    assert!(editor.line().is_empty());
}

#[test]
fn split_escape_crlf_cancel_and_utf8_backspace() {
    let mut editor = Editor::default();
    type_bytes(&mut editor, b"echo ok");
    assert!(matches!(editor.push(b'\r'), Action::Submit(_)));
    assert!(matches!(editor.push(b'\n'), Action::None));
    type_bytes(&mut editor, b"\x1b[");
    assert!(matches!(editor.push(b'A'), Action::None));
    assert!(editor.line().is_empty());
    assert!(matches!(editor.push(3), Action::Cancel));
    type_bytes(&mut editor, "a中".as_bytes());
    editor.push(127);
    assert_eq!(editor.line(), b"a");
    type_bytes(&mut editor, b"\x1b[1;5A");
    assert_eq!(editor.line(), b"a");
    editor.push(21);
    assert!(editor.line().is_empty());
    assert!(matches!(editor.push(4), Action::Exit));
}
