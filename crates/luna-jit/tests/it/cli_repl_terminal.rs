//! The REPL of a `luna` built with the line editor, on a Windows console:
//! the binary runs attached to a pseudo console, so its stdin is a terminal
//! and lines come through the editor. Tab completion is something only the
//! editor does, so a completed name that evaluates shows the editor read
//! the line.

use crate::cli_console::{Console, strip_escapes};

#[test]
fn line_editor_reads_a_completed_line_on_a_console() {
    let console = Console::spawn(&["--lua=5.4", "-i"]);
    let at = console.wait_for("Lua 5.4", 0);
    let at = console.wait_for(">", at);
    // `pri` + Tab completes to `print` from the globals
    console.type_keys("pri\t(6*7)\r");
    let at = console.wait_for("42", at);
    console.type_keys("os.exit(3)\r");
    let code = console.exit_code();
    let screen = console.screen();
    assert_eq!(
        code,
        3,
        "exit status; console shows {:?}",
        screen.get(at..).unwrap_or(&screen)
    );
}

#[test]
fn escapes_are_stripped() {
    assert_eq!(
        strip_escapes("\x1b[2m> \x1b[0mprint\x1b]0;title\x07(1)\x1b[?25h"),
        "> print(1)"
    );
}
