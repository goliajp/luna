//! A Ctrl+Z typed on a Windows console, read by a script and by the REPL:
//! on a console the MSVC C library passes a Ctrl+Z on and ends only the
//! line it is in, and a Ctrl+Z at the start of a line is the end of input.
//!
//! With `LUNA_CONSOLE_PROBE_EXE` set, the test runs that program instead
//! (prefixing the arguments in `LUNA_CONSOLE_PROBE_ARGS`) and prints what
//! each case shows, which is how the expectations were recorded from PUC.

use crate::cli_console::Console;

/// (name, arguments, lines typed): each line is typed once the console
/// shows the text before it, so that what the program writes and what the
/// console echoes land in the same order on every run (the REPL's prompt
/// `> ` must be complete before its line is typed, or the echo splits it).
const CASES: [(&str, &[&str], &[(&str, &str)]); 3] = [
    (
        "read-all",
        &["-e", "\"io.write(('%q'):format(io.read('*a')))\""],
        &[("", "ab\x1acd\r"), ("ab^Zcd", "ef\r"), ("ef", "\x1a\r")],
    ),
    (
        "lines",
        &["-e", "\"for l in io.lines() do io.write('[', l, ']') end\""],
        &[("", "ab\x1acd\r"), ("ab^Zcd", "ef\r"), ("ef", "\x1a\r")],
    ),
    (
        "repl",
        &["-i"],
        &[
            ("> ", "print(1)\x1a2\r"),
            ("print(1)^Z2", "print(3)\r"),
            ("> ", "\x1a\r"),
        ],
    ),
];

/// What the console shows after the case ran, and the exit status.
fn run(
    program: &std::path::Path,
    prefix: &[&str],
    args: &[&str],
    keys: &[(&str, &str)],
) -> (String, u32) {
    let all: Vec<&str> = prefix.iter().chain(args).copied().collect();
    let console = Console::spawn_program(program, &all);
    let mut at = 0;
    for (after, k) in keys {
        if !after.is_empty() {
            at = console.wait_for(after, at);
        }
        console.type_keys(k);
    }
    let code = console.exit_code();
    (console.settled_screen(), code)
}

/// What PUC 5.1.5 to 5.5.0 built with MSVC showed for each case, as the
/// console renders it (`^Z` is the console's echo of Ctrl+Z, `\u{2426}` its
/// glyph for the byte in output); the REPL's version line is left out.
fn puc_screen(dialect: &str, case: &str) -> String {
    let near = match dialect {
        "5.1" => "'char(26)'",
        "5.2" => "char(26)",
        _ => "'<\\26>'",
    };
    let quoted = if dialect == "5.1" {
        "ab\u{2426}ef"
    } else {
        "ab\\26ef"
    };
    match case {
        "read-all" => format!("ab^Zcd\r\nef\r\n^Z\r\n\"{quoted}\\\r\n\""),
        "lines" => "ab^Zcd\r\nef\r\n[ab\u{2426}ef]^Z\r\n".to_string(),
        _ => format!(
            "> print(1)^Z2\r\nprint(3)\r\nstdin:1: unexpected symbol near {near}\r\n> ^Z\r\n\r\n"
        ),
    }
}

#[test]
fn ctrl_z_on_a_console() {
    if let Some(exe) = std::env::var_os("LUNA_CONSOLE_PROBE_EXE") {
        let prefix = std::env::var("LUNA_CONSOLE_PROBE_ARGS").unwrap_or_default();
        let prefix: Vec<&str> = prefix.split_whitespace().collect();
        for (name, args, keys) in CASES {
            let (screen, code) = run(std::path::Path::new(&exe), &prefix, args, keys);
            println!("CASE {name} exit {code}\n{screen:?}\nEND");
        }
        return;
    }
    for d in crate::cli_common::DIALECTS {
        let lua = format!("--lua={d}");
        for (name, args, keys) in CASES {
            let (screen, code) = run(&crate::cli_common::luna(), &[&lua], args, keys);
            let screen = if name == "repl" {
                screen
                    .split_once("\r\n")
                    .map_or(screen.clone(), |(_, rest)| rest.to_string())
            } else {
                screen
            };
            assert_eq!(code, 0, "--lua={d} {name}");
            assert_eq!(screen, puc_screen(d, name), "--lua={d} {name}");
        }
    }
}
