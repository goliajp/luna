//! Host userdata demo — exposing arbitrary `T: 'static` Rust
//! types to Lua via [`Vm::create_userdata`] / [`Vm::set_userdata`],
//! with **v1.2 Track B** `LuaUserdata` trait sugar layered on top so
//! the userdata becomes *callable from Lua* (methods + metamethods).
//!
//! Run: `cargo run --example userdata_demo -p luna-jit`
//!
//! Pairs with `docs/embedding.md` §7.

use luna_core::vm::{LuaUserdata, MetaMethod, UserdataMethods};
use luna_jit::Lua;
use std::time::SystemTime;

/// A toy host type — a counter with read/write semantics. Real
/// embedders expose `DbConn`, `RedisClient`, `AppConfig`, etc.
#[derive(Debug)]
struct Counter {
    value: i64,
    last_updated: SystemTime,
}

impl Counter {
    fn new(initial: i64) -> Self {
        Counter {
            value: initial,
            last_updated: SystemTime::now(),
        }
    }
}

impl LuaUserdata for Counter {
    fn type_name() -> &'static str {
        "Counter"
    }

    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_method("get", |_vm, this, ()| Ok::<_, _>(this.value));
        m.add_method_mut("incr", |_vm, this, (by,): (i64,)| {
            this.value += by;
            this.last_updated = SystemTime::now();
            Ok::<_, _>(())
        });
        m.add_method_mut("set", |_vm, this, (v,): (i64,)| {
            this.value = v;
            this.last_updated = SystemTime::now();
            Ok::<_, _>(())
        });
        m.add_meta_method(MetaMethod::ToString, |_vm, this, ()| {
            Ok::<_, _>(format!("Counter({})", this.value))
        });
    }
}

fn main() {
    let mut lua = Lua::new();
    lua.open_base();

    // 1. Install the host counter as a Lua global. The trait
    //    metatable is built (once per Vm per T) and auto-installed.
    lua.vm()
        .set_userdata("counter", Counter::new(100))
        .expect("install counter");

    // 2. Lua sees `counter` as a userdata value.
    let kind: String = lua.eval("return type(counter)").unwrap();
    assert_eq!(kind, "userdata");
    println!("1. type(counter) = {kind:?}");

    // 3. **NEW in v1.2** — call methods from Lua:
    let v: i64 = lua.eval("return counter:get()").unwrap();
    assert_eq!(v, 100);
    println!("2. counter:get() = {v}");

    lua.eval_multi("counter:incr(50)").unwrap();
    let v: i64 = lua.eval("return counter:get()").unwrap();
    assert_eq!(v, 150);
    println!("3. after counter:incr(50): counter:get() = {v}");

    // 4. `tostring(counter)` routes through the `__tostring` metamethod.
    let s: String = lua.eval("return tostring(counter)").unwrap();
    assert_eq!(s, "Counter(150)");
    println!("4. tostring(counter) = {s:?}");

    // 5. Host-side reads still work — back-compat with v1.1 B8.
    {
        let c: &Counter = lua.vm().userdata_borrow("counter").unwrap();
        assert_eq!(c.value, 150);
        println!("5. host reads: counter.value = {}", c.value);
    }

    // 6. Mutate from the host side.
    {
        let c: &mut Counter = lua.vm().userdata_borrow_mut("counter").unwrap();
        c.value = 999;
        println!("6. host writes: counter.value = 999");
    }
    let v: i64 = lua.eval("return counter:get()").unwrap();
    assert_eq!(v, 999);
    println!("7. after host write: counter:get() = {v}");

    // 7. Downcast typechecks; wrong type returns None.
    {
        let wrong: Option<&String> = lua.vm().userdata_borrow("counter");
        assert!(wrong.is_none());
        println!("8. wrong-type downcast: None (correctly typechecks)");
    }

    // 8. Survives GC.
    lua.vm().collect_garbage();
    let v: i64 = lua.eval("return counter:get()").unwrap();
    assert_eq!(v, 999);
    println!("9. after GC: counter:get() = {v}");

    println!("\nuserdata_demo: all checks passed.");
}
