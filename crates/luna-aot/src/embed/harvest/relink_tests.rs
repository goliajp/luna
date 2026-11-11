use luna_core::__private::CompileOptions;
use luna_core::version::LuaVersion;

use super::super::compile_to_dump;
use super::super::target::TargetSpec;
use super::super::trace_object::build_object_module;
use super::{select_installable, warmup_and_capture};

// a loop whose other way leaves the trace at a guard, the side trace from
// there going on in the trace's loop
const TWO_WAYS: &[u8] = b"local s, t = 0, 0\n\
    for i = 1, 30000 do\n\
      if i % 3 == 0 then s = s + i else t = t + 1 end\n\
    end\n\
    print(s, t)\n";

// AOT code holds no side cells: the warmup's trace has one, filled, but
// the same record lowered for an AOT binary leaves every exit to the
// dispatcher, as before side traces ran from their parents' code
#[test]
fn an_aot_trace_has_no_side_cells() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua54] {
        let td = tempfile::tempdir().expect("tempdir");
        let src = td.path().join("two_ways.lua");
        std::fs::write(&src, TWO_WAYS).expect("write source");
        let dump = compile_to_dump(&src, version).expect("compile source");
        let (_vm, captured) = warmup_and_capture(&dump, version).expect("warmup");
        let installable = select_installable(captured, false);
        assert!(!installable.is_empty(), "{version:?}: no AOT trace");
        let mut module = build_object_module(&TargetSpec::host()).expect("object module");
        for (idx, _, _, record, ct, _) in &installable {
            assert!(
                ct.side_cell_states().iter().any(|&(held, _)| held),
                "{version:?}: the warmup's trace runs no side trace from its code"
            );
            let opts = CompileOptions {
                internal_loop: true,
                pre53: version <= LuaVersion::Lua53,
                aot: true,
                tier: Default::default(),
                tier_up_at: 0,
                dialect: Some(version),
            };
            let name = format!("relink_{idx}");
            let (_, parts) = luna_jit::jit_backend::trace::lower_trace_into_named_for(
                &mut module,
                record,
                opts,
                Some(&name),
                version,
            )
            .expect("AOT lowering");
            assert!(
                parts.side_cell_exits.is_empty(),
                "{version:?}: an AOT side cell"
            );
        }
    }
}
