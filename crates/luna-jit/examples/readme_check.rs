//! Compiles the two snippets in the repository README, so the README
//! cannot drift from the API it advertises.
use luna_jit::Lua;
use luna_jit::Vm;
use luna_jit::version::LuaVersion;

fn first() -> Result<(), Box<dyn std::error::Error>> {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let v = vm.eval("return 6 * 7")?;
    println!("{v:?}");
    Ok(())
}

fn second() -> Result<(), Box<dyn std::error::Error>> {
    let mut lua = Lua::sandbox(LuaVersion::Lua54)
        .open_base()
        .open_math()
        .open_string()
        .with_instr_budget(1_000_000)
        .with_memory_cap(8 * 1024 * 1024)
        .build();

    let add = lua.create_function(|a: i64, b: i64| -> i64 { a + b });
    lua.set_global("add", add)?;

    let result: i64 = lua.eval("return add(40, 2)")?;
    assert_eq!(result, 42);
    Ok(())
}

fn main() {
    first().unwrap();
    second().unwrap();
}
