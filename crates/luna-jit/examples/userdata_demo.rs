//! Host userdata demo (F7) — exposing arbitrary `T: 'static` Rust
//! types to Lua via [`Vm::create_userdata`] / [`Vm::set_userdata`].
//!
//! Run: `cargo run --example userdata_demo -p luna`
//!
//! Pairs with `docs/embedding.md` §7.

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

    fn incr(&mut self, by: i64) {
        self.value += by;
        self.last_updated = SystemTime::now();
    }
}

fn main() {
    let mut lua = Lua::new();
    lua.open_base();

    // 1. Install the host counter as a Lua global.
    lua.vm()
        .set_userdata("counter", Counter::new(100))
        .expect("install counter");

    // 2. Lua sees `counter` as a userdata value (`type(counter) == "userdata"`).
    let kind: String = lua.eval("return type(counter)").unwrap();
    assert_eq!(kind, "userdata");
    println!("1. type(counter) = {kind:?}");

    // 3. The host can read the live state back at any time.
    {
        let c: &Counter = lua.vm().userdata_borrow("counter").unwrap();
        println!("2. host reads: counter.value = {}", c.value);
        assert_eq!(c.value, 100);
    }

    // 4. Mutate from the host side.
    {
        let c: &mut Counter = lua.vm().userdata_borrow_mut("counter").unwrap();
        c.incr(50);
        println!("3. host writes: counter.value += 50 (now {})", c.value);
    }

    // 5. Read back after mutation; demonstrate that the type is
    //    preserved across GC cycles.
    lua.vm().collect_garbage();
    {
        let c: &Counter = lua.vm().userdata_borrow("counter").unwrap();
        assert_eq!(c.value, 150);
        println!(
            "4. after GC + mutation: counter.value = {}",
            c.value
        );
    }

    // 6. Downcast typechecks; wrong type returns None.
    {
        let wrong: Option<&String> = lua.vm().userdata_borrow("counter");
        assert!(wrong.is_none());
        println!("5. wrong-type downcast: None (correctly typechecks)");
    }

    println!("\nuserdata_demo: all checks passed.");
}
