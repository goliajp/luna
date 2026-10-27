//! The registry entries PUC keys by static addresses (5.3's table of C
//! libraries, the debug library's hook table in 5.1 and 5.3) are keyed in
//! luna by fixed light userdata: where they sit in the registry, and so the
//! order a collection traverses what they hold, is the same whatever the
//! build's layout. The listings are those of PUC (5.1.5, 5.3.6, hash seed
//! 1) whose registry keys take the same values, which counts without
//! coincidences of addresses.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = r#"
    debug.sethook(function() end, "r")
    debug.sethook()
    local keys = {}
    for k in pairs(debug.getregistry()) do
      keys[#keys + 1] = type(k) == "userdata" and tostring(k) or type(k)
    end
    return table.concat(keys, " ")
"#;

fn registry_keys(v: LuaVersion) -> String {
    let mut vm = Vm::new_minimal_with_hash_seed(v, 1);
    vm.open_all_libs().expect("the libraries open");
    match vm.eval(SCRIPT).expect("the listing").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{v:?}: got {other:?}"),
    }
}

#[test]
fn static_keys_are_fixed_and_sit_where_they_did() {
    for (v, want) in [(LuaVersion::Lua51, WANT_51), (LuaVersion::Lua53, WANT_53)] {
        assert_eq!(registry_keys(v), want, "{v:?}");
    }
}

const WANT_51: &str = "string string userdata: 0x1020 string";
const WANT_53: &str =
    "number number string userdata: 0x1020 userdata: 0x1010 string string string string";
