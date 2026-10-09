//! 5.1–5.3 code keeps small-number operands in the instruction
//! (`luna_core::vm::isa::imm_form`): one instruction stands for one, the
//! constant table is PUC's, and the bytecode PUC's `luac` writes reads in
//! and writes out unchanged. The `luac` round trip needs `PUC_LUAC_51` …
//! `PUC_LUAC_53`; a dialect without one is skipped with a notice, or fails
//! under `LUNA_DIFF_PUC_REQUIRE_ALL=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use luna_core::runtime::{Gc, Proto, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Op, imm_form};

const DIALECTS: &[(&str, LuaVersion, &str)] = &[
    ("5.1", LuaVersion::Lua51, "PUC_LUAC_51"),
    ("5.2", LuaVersion::Lua52, "PUC_LUAC_52"),
    ("5.3", LuaVersion::Lua53, "PUC_LUAC_53"),
];

/// Every operand form the translation takes, and the ones it must leave:
/// a constant past the immediate range, a float, -0.0, `K - x`, a constant
/// the table holds twice in the other numeric type.
const FORMS: &str = "local a, b = ...
local x = a + 1 local y = a - 1 local z = 1 + a local w = 1 - a
local f = a + 1.5 local g = a + 1000 local h = a - 0.0 local m = -0.0 + a
if a < 1 then x = 0 end if a <= 1 then x = 0 end
if a > 2 then x = 0 end if a >= 2 then x = 0 end
if 3 < a then x = 0 end if 3 >= a then x = 0 end
if a == 4 then x = 0 end if a ~= 4 then x = 0 end
if a == -0.0 then x = 0 end if a < 1.0 then x = 0 end if a < 300 then x = 0 end
if a == 's' then x = 0 end if b > -127 then x = 0 end if b < 128 then x = 0 end
return x, y, z, w, f, g, h, m";

fn fixtures(dialect: &str) -> Vec<PathBuf> {
    // 5.1 has no directory of its own: it takes the 5.2 programs it parses
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/diff_puc")
        .join(if dialect == "5.1" { "5.2" } else { dialect });
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.retain(|p| p.extension().is_some_and(|x| x == "lua"));
    out.sort();
    out
}

fn vm(version: LuaVersion) -> Vm {
    let mut vm = Vm::new(version);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    vm
}

fn dump(vm: &mut Vm, f: Value) -> Vec<u8> {
    let d = vm.eval("return string.dump").expect("string.dump")[0];
    match vm
        .call_value(d, &[f, Value::Bool(false)])
        .expect("dumps")
        .first()
    {
        Some(Value::Str(s)) => s.as_bytes().to_vec(),
        other => panic!("string.dump returned {other:?}"),
    }
}

fn walk(p: Gc<Proto>, out: &mut Vec<Gc<Proto>>) {
    out.push(p);
    for &c in p.protos.iter() {
        walk(c, out);
    }
}

/// Every instruction of `p` comes back from its constant form, and the
/// constant form names a constant the table has: one instruction stands
/// for one, and the table is the one PUC writes.
fn assert_one_for_one(what: &str, p: Gc<Proto>) {
    let mut all = Vec::new();
    walk(p, &mut all);
    for p in all {
        for (pc, &i) in p.code.iter().enumerate() {
            let k = imm_form::to_k(i, &p.consts)
                .unwrap_or_else(|| panic!("{what} pc {pc}: {i:?} has no constant form"));
            assert_eq!(imm_form::to_imm(k, &p.consts), i, "{what} pc {pc}: {k:?}");
            let named = match k.op() {
                Op::AddK | Op::SubK => Some(k.c()),
                Op::LtK | Op::LeK | Op::EqK => Some(k.b()),
                _ => None,
            };
            assert!(
                named.is_none_or(|n| (n as usize) < p.consts.len()),
                "{what} pc {pc}: {k:?}"
            );
        }
    }
}

/// Compiled, dumped, loaded and dumped again: the same bytes, and every
/// function one for one both ways. Gives the compiled code's opcodes.
fn round_trip(version: LuaVersion, what: &str, src: &[u8]) -> Option<Vec<Op>> {
    let mut vm = vm(version);
    let f = vm.load(src, b"=x").ok()?;
    assert_one_for_one(what, f.proto);
    let bytes = dump(&mut vm, Value::Closure(f));
    let g = vm.load(&bytes, b"=x").expect("the dump loads");
    assert_one_for_one(what, g.proto);
    assert_eq!(
        dump(&mut vm, Value::Closure(g)),
        bytes,
        "{what}: dump of the loaded dump"
    );
    let mut all = Vec::new();
    walk(f.proto, &mut all);
    Some(
        all.iter()
            .flat_map(|p| p.code.iter().map(|i| i.op()))
            .collect(),
    )
}

#[test]
fn small_number_operands_stay_one_instruction_and_round_trip() {
    for &(dialect, version, _) in DIALECTS {
        let ops = round_trip(version, dialect, FORMS.as_bytes()).expect("FORMS compiles");
        // the forms the translation makes, so the test sees it at work
        let want: &[Op] = if version == LuaVersion::Lua53 {
            &[
                Op::AddI,
                Op::SubI,
                Op::LtI,
                Op::LeI,
                Op::GtI,
                Op::GeI,
                Op::EqI,
            ]
        } else {
            &[Op::LtI, Op::LeI, Op::GtI, Op::GeI, Op::EqI]
        };
        for op in want {
            assert!(ops.contains(op), "{dialect}: no {op:?} in {ops:?}");
        }
        for path in fixtures(dialect) {
            let src = std::fs::read(&path).expect("fixture");
            round_trip(version, &format!("{dialect} {}", path.display()), &src);
        }
    }
}

fn luac(bin: &str, src: &Path, out: &Path) -> bool {
    Command::new(bin)
        .arg("-o")
        .arg(out)
        .arg(src)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn luac_bytecode_reads_in_and_writes_out_unchanged() {
    let require = std::env::var_os("LUNA_DIFF_PUC_REQUIRE_ALL").is_some();
    let out = std::env::temp_dir().join(format!("luna-imm-form-{}.luac", std::process::id()));
    let src = std::env::temp_dir().join(format!("luna-imm-form-{}.lua", std::process::id()));
    std::fs::write(&src, FORMS).expect("temp source");
    let mut failed = Vec::new();
    for &(dialect, version, env) in DIALECTS {
        let Ok(bin) = std::env::var(env) else {
            assert!(!require, "{env} is not set");
            eprintln!("imm_form: {env} not set, {dialect} skipped");
            continue;
        };
        let mut files = fixtures(dialect);
        files.push(src.clone());
        let mut n = 0;
        for path in files {
            if !luac(&bin, &path, &out) {
                continue;
            }
            let bytes = std::fs::read(&out).expect("luac output");
            let mut vm = vm(version);
            let f = vm.load(&bytes, b"=x").expect("luac output loads");
            assert_one_for_one(dialect, f.proto);
            let again = dump(&mut vm, Value::Closure(f));
            // luna writes a 5.1 main function's vararg flag as 3 where luac
            // writes 2: from 5.1 the bytes must come back the second time
            let back = if version == LuaVersion::Lua51 {
                let g = vm.load(&again, b"=x").expect("the dump loads");
                dump(&mut vm, Value::Closure(g)) == again
            } else {
                again == bytes
            };
            if !back {
                failed.push(format!("{dialect} {}", path.display()));
            }
            n += 1;
        }
        assert!(n > 1, "{dialect}: luac compiled {n} files");
    }
    let _ = (std::fs::remove_file(&out), std::fs::remove_file(&src));
    assert!(
        failed.is_empty(),
        "written back differently:\n{}",
        failed.join("\n")
    );
}
