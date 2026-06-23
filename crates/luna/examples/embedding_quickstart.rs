//! Embedding quickstart (F7) — the cookbook's "Hello, world" plus the
//! five most common embedder patterns, in a single self-contained file.
//!
//! Run: `cargo run --example embedding_quickstart -p luna`
//!
//! Pairs with `docs/embedding.md` §§2-6.

use luna::Lua;
use luna::version::LuaVersion;

fn main() {
    // 1. Plain Lua with full stdlib + JIT on.
    {
        let mut lua = Lua::new();
        lua.open_base();
        let r: i64 = lua.eval("return 1 + 2").unwrap();
        assert_eq!(r, 3);
        println!("1. Hello: 1 + 2 = {r}");
    }

    // 2. Sandbox builder for untrusted scripts.
    {
        let mut lua = Lua::sandbox(LuaVersion::Lua54)
            .open_base()
            .open_math()
            .open_string()
            .with_instr_budget(1_000_000)
            .with_memory_cap(8 * 1024 * 1024)
            .build();
        let r: f64 = lua.eval("return math.sqrt(16) + 0.5").unwrap();
        assert!((r - 4.5).abs() < 1e-9);
        println!("2. Sandbox: math.sqrt(16) + 0.5 = {r}");
    }

    // 3. Globals (IntoValue covers the common primitives).
    {
        let mut lua = Lua::new();
        lua.open_base();
        lua.set_global("count", 42_i64).unwrap();
        lua.set_global("name", "luna").unwrap();
        let r: String = lua.eval("return name .. ':' .. count").unwrap();
        assert_eq!(r, "luna:42");
        println!("3. Globals: {r}");
    }

    // 4. Tables — TableBuilder + table_of.
    {
        let mut lua = Lua::new();
        lua.open_base();
        let t = lua.create_table();
        t.set(&mut lua, "answer", 42_i64).unwrap();
        t.set(&mut lua, "name", "luna").unwrap();
        lua.set_global("c", t).unwrap();
        let r: i64 = lua.eval("return c.answer").unwrap();
        assert_eq!(r, 42);
        println!("4. Table: c.answer = {r}");
    }

    // 5. Native functions — typed Rust closure callable from Lua.
    {
        let mut lua = Lua::new();
        lua.open_base();
        let add = lua.create_function(|a: i64, b: i64| -> i64 { a + b });
        lua.set_global("add", add).unwrap();
        let r: i64 = lua.eval("return add(40, 2)").unwrap();
        assert_eq!(r, 42);
        println!("5. Native: add(40, 2) = {r}");
    }

    println!("\nembedding_quickstart: all checks passed.");
}
