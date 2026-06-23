//! v1.2 Track B — dogfood §4.1 redis-stub-shape rewrite.
//!
//! The kevy v1.0 dogfood report (`.dev/dogfood/2026-06-23-kevy/`) flagged
//! the "3 steps + 1 unsafe block per table entry" pattern of installing
//! a populated dispatch table:
//!
//! ```ignore
//! // v1.1 — what the dogfood actually wrote:
//! let t = vm.heap.new_table();
//! let key = Value::Str(vm.heap.intern(b"call"));
//! let func = vm.native(redis_call);
//! unsafe { t.as_mut() }
//!     .set(&mut vm.heap, key, func)
//!     .expect("redis method registration");
//! // ... × 7 entries
//! vm.set_global("redis", Value::Table(t));
//! ```
//!
//! Plus the dispatch fn itself had to use the raw
//! `fn(vm, fs, nargs) -> Result<u32, LuaError>` shape, with
//! `nat_arg(fs, nargs, i)` for each positional arg and an external
//! `thread_local!` to hold mutable state (because `vm.native` only
//! accepted ZST `Copy` closures, no captures).
//!
//! This example shows the v1.2 LuaUserdata trait equivalent: the
//! state IS the userdata payload, methods receive `&mut Self`, args
//! arrive as a typed `Vec<Value>`, and the install is one line.
//!
//! Run: `cargo run --example userdata_redis_stub -p luna-jit`

use luna_core::runtime::Value;
use luna_core::vm::{LuaError, LuaUserdata, UserdataMethods};
use luna_jit::Lua;
use std::collections::HashMap;

#[derive(Default)]
struct FakeRedis {
    strings: HashMap<Vec<u8>, Vec<u8>>,
    call_log: Vec<Vec<Vec<u8>>>,
}

impl FakeRedis {
    fn dispatch(&mut self, args: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
        self.call_log.push(args.clone());
        let cmd: Vec<u8> = args[0].iter().map(|b| b.to_ascii_uppercase()).collect();
        match cmd.as_slice() {
            b"SET" => {
                if args.len() < 3 {
                    return Err("wrong number of arguments for 'set'".into());
                }
                self.strings.insert(args[1].clone(), args[2].clone());
                Ok(b"OK".to_vec())
            }
            b"GET" => {
                if args.len() < 2 {
                    return Err("wrong number of arguments for 'get'".into());
                }
                Ok(self.strings.get(&args[1]).cloned().unwrap_or_default())
            }
            _ => Err(format!(
                "unknown command: {}",
                String::from_utf8_lossy(&cmd)
            )),
        }
    }
}

fn val_to_bytes(v: Value) -> Vec<u8> {
    match v {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(i) => i.to_string().into_bytes(),
        Value::Float(f) => format!("{f}").into_bytes(),
        Value::Bool(true) => b"1".to_vec(),
        Value::Bool(false) | Value::Nil => Vec::new(),
        _ => format!("{v:?}").into_bytes(),
    }
}

impl LuaUserdata for FakeRedis {
    fn type_name() -> &'static str {
        "FakeRedis"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_method_mut(
            "call",
            |vm, this, args: Vec<Value>| -> Result<Value, LuaError> {
                let argv: Vec<Vec<u8>> = args.into_iter().map(val_to_bytes).collect();
                if argv.is_empty() {
                    return Err(vm.rt_err("redis.call: missing command"));
                }
                match this.dispatch(argv) {
                    Ok(bytes) => {
                        let s = vm.intern_str(std::str::from_utf8(&bytes).unwrap_or(""));
                        Ok(Value::Str(s))
                    }
                    Err(msg) => Err(vm.rt_err(&msg)),
                }
            },
        );
        m.add_method_mut(
            "pcall",
            |vm, this, args: Vec<Value>| -> Result<Value, LuaError> {
                let argv: Vec<Vec<u8>> = args.into_iter().map(val_to_bytes).collect();
                match this.dispatch(argv) {
                    Ok(bytes) => {
                        let s = vm.intern_str(std::str::from_utf8(&bytes).unwrap_or(""));
                        Ok(Value::Str(s))
                    }
                    Err(_) => Ok(Value::Nil),
                }
            },
        );
        m.add_method("log_size", |_vm, this, ()| {
            Ok::<_, LuaError>(this.call_log.len() as i64)
        });
        m.add_method("status_reply", |vm, _this, (s,): (String,)| {
            Ok::<_, LuaError>(Value::Str(vm.intern_str(&s)))
        });
    }
}

fn main() {
    let mut lua = Lua::new();
    lua.open_base();

    // 1-line install (v1.1 was ~10 LOC + 1 unsafe block per entry).
    lua.vm()
        .set_userdata("redis", FakeRedis::default())
        .unwrap();

    lua.eval_multi(
        r#"
        redis:call("SET", "name", "luna")
        redis:call("SET", "version", "1.2")
    "#,
    )
    .unwrap();

    let v: String = lua.eval("return redis:call('GET', 'name')").unwrap();
    assert_eq!(v, "luna");
    println!("1. GET name → {v:?}");

    let v: String = lua.eval("return redis:call('GET', 'version')").unwrap();
    assert_eq!(v, "1.2");
    println!("2. GET version → {v:?}");

    let n: i64 = lua.eval("return redis:log_size()").unwrap();
    assert_eq!(n, 4);
    println!("3. log_size = {n} (3 SET/GET + 1 GET)");

    let v: String = lua.eval("return redis:status_reply('READY')").unwrap();
    assert_eq!(v, "READY");
    println!("4. status_reply → {v:?}");

    // pcall — wrong arity returns nil rather than erroring.
    let r = lua.vm().eval("return redis:pcall('SET')").unwrap();
    assert!(matches!(r[0], Value::Nil));
    println!("5. pcall('SET') → nil (error suppressed)");

    // Host-side inspection of the state — still works.
    let host: &FakeRedis = lua.vm().userdata_borrow("redis").unwrap();
    assert_eq!(host.strings.len(), 2);
    assert!(host.call_log.len() >= 4);
    println!(
        "6. host inspect: {} keys, {} calls logged",
        host.strings.len(),
        host.call_log.len()
    );

    println!("\nuserdata_redis_stub: all checks passed.");
    println!("\nLOC of the embedder-side install + dispatch wiring:");
    println!("  v1.1 raw shape (dogfood §4.1): ~10 LOC + 1 unsafe per entry");
    println!("                                 + thread_local for mutable state");
    println!("  v1.2 trait shape (this file):  1-line install + Vec<Value> args");
    println!("                                 + state IS the userdata payload");
}
