//! A shared trace is installed only for code whose content is the same,
//! not just whose content hash is.

use crate::Engine;
use luna_core::version::LuaVersion;

const SRC: &str = r#"
local t = { n = 0 }
for i = 1, 400 do t.n = t.n + i % 7 end
return t.n
"#;

fn vm(engine: &Engine) -> luna_core::vm::Vm {
    let mut vm = engine.new_vm(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = 8;
    vm
}

#[test]
fn a_trace_for_other_content_with_the_same_hash_is_not_installed() {
    let engine = Engine::new();
    {
        let mut a = vm(&engine);
        a.eval(SRC).expect("eval");
        assert!(a.trace_compiled_count() > 0);
    }
    // the first Vm is gone, so the cache holds the only reference
    {
        let mut cache = engine.cache();
        let mut forged = 0;
        for list in cache.by_head.values_mut() {
            for img in list.iter_mut() {
                let img = std::sync::Arc::get_mut(img).expect("only the cache holds it");
                let mut bytes = img.protos[0].bytes.to_vec();
                bytes[0] ^= 1;
                img.protos[0].bytes = bytes.into();
                forged += 1;
            }
        }
        assert!(forged > 0);
    }
    let mut b = vm(&engine);
    let r = b.eval(SRC).expect("eval");
    assert!(matches!(r[0], luna_core::runtime::Value::Int(_)));
    assert_eq!(b.trace_adopted_count(), 0, "a forged trace was installed");
    assert!(b.trace_compiled_count() > 0);
}
