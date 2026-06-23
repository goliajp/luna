//! `repl_lite` (F7) — a 50-line REPL embeddable into a host program.
//! Shows how `Vm::eval` composes with stdin / stdout pumping when
//! the embedder owns the read loop (vs. handing control to the
//! shipped `luna` binary's full REPL).
//!
//! Run: `cargo run --example repl_lite -p luna` then type Lua at
//! the `lite>` prompt. Ctrl-D / EOF exits.
//!
//! Pairs with the `luna` CLI binary's C1 REPL — that one lives in
//! `crates/luna/src/bin/luna.rs`, this is the embedder-side
//! equivalent for hosts that want a Lua console inside their own
//! application.

use luna_jit::Lua;
use luna_jit::runtime::Value;
use std::io::{BufRead, Write};

fn render(v: Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Str(s) => format!("{:?}", String::from_utf8_lossy(s.as_bytes())),
        Value::Table(_) => "<table>".into(),
        Value::Closure(_) | Value::Native(_) => "<function>".into(),
        Value::Coro(_) => "<thread>".into(),
        Value::Userdata(_) => "<userdata>".into(),
        Value::LightUserdata(_) => "<lightuserdata>".into(),
    }
}

fn main() {
    let mut lua = Lua::new();
    lua.open_base();
    lua.open_math();
    lua.open_string();

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut buf = String::new();

    println!("repl_lite: embedded Lua REPL. Ctrl-D to exit.");

    loop {
        print!("lite> ");
        let _ = stdout.flush();
        buf.clear();
        match stdin.lock().read_line(&mut buf) {
            Ok(0) => {
                println!();
                return;
            }
            Ok(_) => {
                let line = buf.trim();
                if line.is_empty() {
                    continue;
                }
                // Expression-first; fall back to statement on syntax error.
                let as_expr = format!("return {line}");
                let result = match lua.vm().eval(&as_expr) {
                    Ok(vs) => Ok(vs),
                    Err(_) => lua.vm().eval(line),
                };
                match result {
                    Ok(vs) => {
                        for v in vs {
                            println!("{}", render(v));
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {}", lua.vm().error_text(&e));
                    }
                }
            }
            Err(e) => {
                eprintln!("io error: {e}");
                return;
            }
        }
    }
}
