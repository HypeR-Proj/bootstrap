// SPDX-FileCopyrightText: 2026 roolrz
// SPDX-License-Identifier: Apache-2.0

use super::{CommandLine, ParseError};

#[test]
fn parses_quotes_escapes_and_empty_arguments() -> Result<(), ParseError> {
    let command = CommandLine::parse(br#"echo "two words" 'three' four\ five """#)?;
    assert!(!command.is_empty());
    assert_eq!(command.len(), 5);
    assert_eq!(command.argument(0), Some("echo"));
    assert_eq!(command.argument(1), Some("two words"));
    assert_eq!(command.argument(2), Some("three"));
    assert_eq!(command.argument(3), Some("four five"));
    assert_eq!(command.argument(4), Some(""));
    Ok(())
}

#[test]
fn recognizes_an_empty_command_line() -> Result<(), ParseError> {
    let command = CommandLine::parse(b" \t ")?;
    assert!(command.is_empty());
    assert_eq!(command.len(), 0);
    Ok(())
}

#[test]
fn rejects_oversized_lines_arguments_and_nul() {
    assert!(matches!(
        CommandLine::parse(&vec![b'x'; super::MAX_LINE_BYTES + 1]),
        Err(ParseError::LineTooLong)
    ));
    let too_many = vec!["x"; super::MAX_ARGUMENTS + 1].join(" ");
    assert!(matches!(
        CommandLine::parse(too_many.as_bytes()),
        Err(ParseError::TooManyArguments)
    ));
    assert!(matches!(
        CommandLine::parse(b"echo a\0b"),
        Err(ParseError::InvalidSyntax)
    ));
}

#[test]
fn follows_posix_comments_and_double_quote_escaping() -> Result<(), ParseError> {
    let command = CommandLine::parse(br#"echo "a\qb" '# literal' # comment"#)?;
    assert_eq!(
        command.arguments().collect::<Vec<_>>(),
        ["echo", r"a\qb", "# literal"]
    );
    Ok(())
}

#[test]
fn rejects_incomplete_syntax() {
    assert!(matches!(
        CommandLine::parse(b"echo 'missing"),
        Err(ParseError::InvalidSyntax)
    ));
    assert!(matches!(
        CommandLine::parse(b"echo trailing\\"),
        Err(ParseError::InvalidSyntax)
    ));
}
