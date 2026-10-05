//! A Ctrl+Z typed on a Windows console, read by a script and by the REPL:
//! on a console the MSVC C library passes a Ctrl+Z on and ends only the
//! line it is in, and a Ctrl+Z at the start of a line is the end of input.
//!
//! With `LUNA_CONSOLE_PROBE_EXE` set, the test runs that program instead
//! (prefixing the arguments in `LUNA_CONSOLE_PROBE_ARGS`) and prints what
//! each case shows, which is how the expectations were recorded from PUC.

use crate::cli_console::Console;

/// (name, arguments, lines typed)
const CASES: [(&str, &[&str], &[&str]); 3] = [
    (
        "read-all",
        &["-e", "\"io.write(('%q'):format(io.read('*a')))\""],
        &["ab\x1acd\r", "ef\r", "\x1a\r"],
    ),
    (
        "lines",
        &["-e", "\"for l in io.lines() do io.write('[', l, ']') end\""],
        &["ab\x1acd\r", "ef\r", "\x1a\r"],
    ),
    (
        "repl",
        &["-i"],
        &["print(1)\x1a2\r", "print(3)\r", "\x1a\r"],
    ),
];

/// What the console shows after the case ran, and the exit status.
fn run(program: &std::path::Path, prefix: &[&str], args: &[&str], keys: &[&str]) -> (String, u32) {
    let all: Vec<&str> = prefix.iter().chain(args).copied().collect();
    let console = Console::spawn_program(program, &all);
    for k in keys {
        std::thread::sleep(std::time::Duration::from_millis(300));
        console.type_keys(k);
    }
    let code = console.exit_code();
    std::thread::sleep(std::time::Duration::from_millis(300));
    (console.screen(), code)
}

#[test]
fn ctrl_z_on_a_console() {
    let Some(exe) = std::env::var_os("LUNA_CONSOLE_PROBE_EXE") else {
        return;
    };
    let prefix = std::env::var("LUNA_CONSOLE_PROBE_ARGS").unwrap_or_default();
    let prefix: Vec<&str> = prefix.split_whitespace().collect();
    for (name, args, keys) in CASES {
        let (screen, code) = run(std::path::Path::new(&exe), &prefix, args, keys);
        println!("CASE {name} exit {code}\n{screen:?}\nEND");
    }
}
