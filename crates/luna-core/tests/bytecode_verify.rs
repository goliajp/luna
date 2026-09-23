//! The bytecode verifier: a binary chunk that breaks an invariant the VM
//! relies on is refused at load time with `bad binary format (...)`.
//!
//! Each case compiles a small function, dumps it with `string.dump`, edits
//! one field of the dumped prototype and loads the result. The edits use a
//! test-side decoder for luna's own body format (see `vm/dump/luna.rs`);
//! chunks from the PUC translators reach the same check after lowering.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

/// PUC 5.5 header (40 bytes) + luna body tag (8 bytes).
const BODY_AT: usize = 48;

/// One prototype of luna's dump body; constants, upvalue names and local
/// records are kept as raw bytes since no case edits them.
#[derive(Clone)]
struct P {
    num_params: u8,
    is_vararg: u8,
    max_stack: u8,
    line_defined: u32,
    last_line: u32,
    source: Vec<u8>,
    code: Vec<u32>,
    lines: Vec<u32>,
    n_consts: u32,
    consts: Vec<u8>,
    upvals: Vec<(u8, u8, u8, Vec<u8>)>,
    protos: Vec<P>,
    locvars: Vec<u8>,
}

struct Rd<'a>(&'a [u8], usize);

impl Rd<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.0[self.1..self.1 + n];
        self.1 += n;
        s
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn bytes(&mut self) -> Vec<u8> {
        let n = self.u32() as usize;
        self.take(n).to_vec()
    }
    fn proto(&mut self) -> P {
        let num_params = self.u8();
        let is_vararg = self.u8();
        let max_stack = self.u8();
        let line_defined = self.u32();
        let last_line = self.u32();
        let source = self.bytes();
        let code = (0..self.u32()).map(|_| self.u32()).collect();
        let lines = (0..self.u32()).map(|_| self.u32()).collect();
        let n_consts = self.u32();
        let start = self.1;
        for _ in 0..n_consts {
            match self.u8() {
                0..=2 => {}
                3 | 4 => {
                    self.take(8);
                }
                5 => {
                    self.bytes();
                }
                6 => {
                    self.u32();
                }
                t => panic!("constant tag {t}"),
            }
        }
        let consts = self.0[start..self.1].to_vec();
        let upvals = (0..self.u32())
            .map(|_| (self.u8(), self.u8(), self.u8(), self.bytes()))
            .collect();
        let protos = (0..self.u32()).map(|_| self.proto()).collect();
        let start = self.1;
        for _ in 0..self.u32() {
            self.bytes();
            self.take(12);
        }
        let locvars = self.0[start..self.1].to_vec();
        P {
            num_params,
            is_vararg,
            max_stack,
            line_defined,
            last_line,
            source,
            code,
            lines,
            n_consts,
            consts,
            upvals,
            protos,
            locvars,
        }
    }
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write(p: &P, out: &mut Vec<u8>) {
    out.extend_from_slice(&[p.num_params, p.is_vararg, p.max_stack]);
    put32(out, p.line_defined);
    put32(out, p.last_line);
    put32(out, p.source.len() as u32);
    out.extend_from_slice(&p.source);
    put32(out, p.code.len() as u32);
    p.code.iter().for_each(|&w| put32(out, w));
    put32(out, p.lines.len() as u32);
    p.lines.iter().for_each(|&l| put32(out, l));
    put32(out, p.n_consts);
    out.extend_from_slice(&p.consts);
    put32(out, p.upvals.len() as u32);
    for (s, i, r, name) in &p.upvals {
        out.extend_from_slice(&[*s, *i, *r]);
        put32(out, name.len() as u32);
        out.extend_from_slice(name);
    }
    put32(out, p.protos.len() as u32);
    p.protos.iter().for_each(|c| write(c, out));
    out.extend_from_slice(&p.locvars);
}

/// `string.dump` of the function `src` returns, split into header + body.
fn dumped(src: &str) -> (Vec<u8>, P) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let code = format!("return string.dump((function() {src} end)())");
    let v = vm.eval(&code).expect("dump");
    let Value::Str(s) = v[0] else {
        panic!("string.dump returned {:?}", v[0]);
    };
    let bytes = s.as_bytes().to_vec();
    let mut rd = Rd(&bytes, BODY_AT);
    let p = rd.proto();
    assert_eq!(
        rd.1,
        bytes.len(),
        "test decoder out of step with the format"
    );
    (bytes[..BODY_AT].to_vec(), p)
}

fn chunk(header: &[u8], p: &P) -> Vec<u8> {
    let mut out = header.to_vec();
    write(p, &mut out);
    out
}

fn load_err(bytes: &[u8]) -> String {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.load(bytes, b"=crafted") {
        Ok(_) => panic!("crafted chunk loaded"),
        Err(e) => e.msg_str().into_owned(),
    }
}

/// Load `mutate`d `src`, expecting a verifier error containing `want`.
#[track_caller]
fn refused(src: &str, mutate: impl FnOnce(&mut P), want: &str) {
    let (header, mut p) = dumped(src);
    // the unedited chunk, re-encoded by the test decoder, still loads
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.load(&chunk(&header, &p), b"=ok")
        .expect("pristine chunk");
    mutate(&mut p);
    let msg = load_err(&chunk(&header, &p));
    assert!(
        msg.starts_with("bad binary format (") && msg.contains(want),
        "unexpected error: {msg}"
    );
}

/// First pc of `op` in `p`.
fn find(p: &P, op: Op) -> usize {
    p.code
        .iter()
        .position(|&w| Inst(w).op() == op)
        .unwrap_or_else(|| panic!("no {op:?} in the compiled function"))
}

/// Rewrite the instruction at `pc` through its field accessors.
fn set(p: &mut P, pc: usize, f: impl FnOnce(Inst) -> Inst) {
    p.code[pc] = f(Inst(p.code[pc])).0;
}

fn with_a(i: Inst, a: u32) -> Inst {
    Inst((i.0 & !(0xFF << 7)) | (a << 7))
}
fn with_b(i: Inst, b: u32) -> Inst {
    Inst((i.0 & !(0xFF << 16)) | (b << 16))
}
fn with_c(i: Inst, c: u32) -> Inst {
    Inst((i.0 & !(0xFF << 24)) | (c << 24))
}
fn with_bx(i: Inst, bx: u32) -> Inst {
    Inst((i.0 & 0x7FFF) | (bx << 15))
}

const ADD: &str = "return function(x, y) local z = x + y return z end";

#[test]
fn invalid_opcode() {
    refused(
        ADD,
        |p| p.code[0] = (p.code[0] & !0x7F) | 0x7F,
        "invalid opcode 127",
    );
}

#[test]
fn register_beyond_max_stack() {
    refused(
        ADD,
        |p| {
            let pc = find(p, Op::Add);
            set(p, pc, |i| with_a(i, 200));
        },
        "register 200 out of range",
    );
}

#[test]
fn register_run_beyond_max_stack() {
    refused(
        "return function() local a, b, c return a end",
        |p| {
            let pc = find(p, Op::LoadNil);
            set(p, pc, |i| with_b(i, 250));
        },
        "out of range (stack size",
    );
}

#[test]
fn call_results_beyond_max_stack() {
    refused(
        "return function() local f = print f() end",
        |p| {
            let pc = find(p, Op::Call);
            set(p, pc, |i| with_c(i, 100));
        },
        "out of range (stack size",
    );
}

#[test]
fn constant_index_out_of_range() {
    refused(
        "return function() return 'k' end",
        |p| {
            let pc = find(p, Op::LoadK);
            set(p, pc, |i| with_bx(i, 999));
        },
        "constant 999 out of range",
    );
}

#[test]
fn upvalue_index_out_of_range() {
    refused(
        "local u = 1 return function() return u end",
        |p| {
            let pc = find(p, Op::GetUpval);
            set(p, pc, |i| with_b(i, 9));
        },
        "upvalue 9 out of range",
    );
}

#[test]
fn closure_index_out_of_range() {
    refused(
        "return function() return function() end end",
        |p| {
            let pc = find(p, Op::Closure);
            set(p, pc, |i| with_bx(i, 5));
        },
        "function 5 out of range",
    );
}

#[test]
fn jump_outside_the_code() {
    refused(
        "return function(x) if x then x = 1 end return x end",
        |p| {
            let pc = find(p, Op::Jmp);
            p.code[pc] = Inst::isj(Op::Jmp, 1000).0;
        },
        "jumps outside the code",
    );
}

#[test]
fn falls_off_the_end() {
    refused(
        ADD,
        |p| {
            let last = p.code.len() - 1;
            p.code[last] = Inst::iabc(Op::Move, 0, 0, 0, false).0;
        },
        "falls off the end of the code",
    );
}

#[test]
fn empty_code() {
    refused(
        ADD,
        |p| {
            p.code.clear();
            p.lines.clear();
        },
        "no instructions",
    );
}

#[test]
fn loadkx_without_extra_arg() {
    refused(
        "return function() return 'k' end",
        |p| {
            let pc = find(p, Op::LoadK);
            let a = Inst(p.code[pc]).a();
            p.code[pc] = Inst::iabx(Op::LoadKx, a, 0).0;
        },
        "not followed by its extra argument",
    );
}

#[test]
fn extra_arg_executed() {
    refused(
        ADD,
        |p| p.code[0] = Inst::iax(Op::ExtraArg, 0).0,
        "executed as an instruction",
    );
}

#[test]
fn for_prep_without_its_loop() {
    refused(
        "return function() local s = 0 for i = 1, 3 do s = s + i end return s end",
        |p| {
            let pc = find(p, Op::ForPrep);
            set(p, pc, |i| with_bx(i, i.bx() + 1));
        },
        "no matching ForLoop",
    );
}

#[test]
fn for_loop_without_its_prep() {
    refused(
        "return function() local s = 0 for i = 1, 3 do s = s + i end return s end",
        |p| {
            let pc = find(p, Op::ForPrep);
            p.code[pc] = Inst::isj(Op::Jmp, 0).0;
        },
        "not paired with a ForPrep",
    );
}

#[test]
fn tforcall_not_followed_by_tforloop() {
    refused(
        "return function(t) for k in pairs(t) do end end",
        |p| {
            let pc = find(p, Op::TForLoop);
            p.code[pc] = Inst::iabc(Op::Return0, 0, 0, 0, false).0;
        },
        "TForLoop",
    );
}

#[test]
fn tforloop_without_its_tforcall() {
    refused(
        "return function(t) for k in pairs(t) do end end",
        |p| {
            // a second TForLoop, looping on itself, before the final return:
            // no TForCall precedes it
            let a = Inst(p.code[find(p, Op::TForCall)]).a();
            let at = p.code.len() - 1;
            p.code.insert(at, Inst::iabx(Op::TForLoop, a, 1).0);
            p.lines.insert(at, 1);
        },
        "not preceded by its TForCall",
    );
}

#[test]
fn compare_not_followed_by_jmp() {
    refused(
        "return function(x, y) if x < y then return 1 end return 2 end",
        |p| {
            let pc = find(p, Op::Lt);
            p.code[pc + 1] = Inst::iabc(Op::Move, 0, 0, 0, false).0;
        },
        "not followed by a Jmp",
    );
}

#[test]
fn open_call_without_producer() {
    refused(
        "return function(...) return print(...) end",
        |p| {
            let pc = find(p, Op::Vararg);
            // results fixed at one value: the B = 0 consumer has no top
            set(p, pc, |i| with_c(i, 2));
        },
        "stack top no instruction set",
    );
}

#[test]
fn line_info_length_mismatch() {
    refused(ADD, |p| p.lines.push(1), "line entries for");
}

#[test]
fn params_exceed_max_stack() {
    refused(
        ADD,
        |p| p.num_params = p.max_stack + 1,
        "parameters exceed stack size",
    );
}

#[test]
fn nested_upvalue_descriptor_out_of_range() {
    refused(
        "return function() local u = 1 return function() return u end end",
        |p| p.protos[0].upvals[0].1 = 250,
        "captures register 250 out of range",
    );
}

#[test]
fn nested_function_is_checked() {
    refused(
        "return function() return function(x, y) return x + y end end",
        |p| {
            let pc = find(&p.protos[0], Op::Add);
            set(&mut p.protos[0], pc, |i| with_b(i, 222));
        },
        "function at line",
    );
}

/// Load-time cost over the corpus: every diff_puc 5.5 fixture and 5.5
/// official test file, dumped by luna and (when `PUC_LUAC_55` is set)
/// compiled by PUC's `luac`, loaded `ROUNDS` times per sample. Run the same
/// test on a tree without the verifier to get the overhead:
///
///     cargo test --release -p luna-core --test bytecode_verify \
///         load_overhead -- --ignored --nocapture
#[test]
#[ignore = "measurement, not a check"]
fn load_overhead() {
    use std::time::Instant;
    const ROUNDS: usize = 20;
    const SAMPLES: usize = 15;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut sources = Vec::new();
    for dir in ["diff_puc/5.5", "official/lua-5.5.1-tests"] {
        let mut paths: Vec<_> = std::fs::read_dir(root.join(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "lua"))
            .collect();
        paths.sort();
        sources.extend(paths);
    }
    let mut luna_chunks = Vec::new();
    let mut vm = Vm::new(LuaVersion::Lua55);
    let dump = vm.eval("return string.dump").unwrap()[0];
    for path in &sources {
        // a file luna refuses to compile as 5.5 is left out
        let Ok(f) = vm.load(&std::fs::read(path).unwrap(), b"=bench") else {
            continue;
        };
        let v = vm.call_value(dump, &[Value::Closure(f)]).unwrap();
        let Value::Str(s) = v[0] else {
            panic!("string.dump returned {:?}", v[0]);
        };
        luna_chunks.push(s.as_bytes().to_vec());
    }
    let mut puc_chunks = Vec::new();
    if let Ok(luac) = std::env::var("PUC_LUAC_55") {
        let tmp = std::env::temp_dir().join("luna-verify-bench.luac");
        for path in &sources {
            let ok = std::process::Command::new(&luac)
                .arg("-o")
                .arg(&tmp)
                .arg(path)
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                puc_chunks.push(std::fs::read(&tmp).unwrap());
            }
        }
    }
    for (label, chunks, puc) in [("luna", &luna_chunks, false), ("puc", &puc_chunks, true)] {
        if chunks.is_empty() {
            continue;
        }
        let bytes: usize = chunks.iter().map(Vec::len).sum();
        let mut vm = Vm::new(LuaVersion::Lua55);
        vm.set_puc_bytecode_loading(puc);
        let mut loaded = 0;
        for c in chunks.iter() {
            loaded += vm.load(c, b"=bench").is_ok() as usize;
        }
        let mut times = Vec::new();
        for _ in 0..SAMPLES {
            let t = Instant::now();
            for _ in 0..ROUNDS {
                for c in chunks.iter() {
                    let _ = std::hint::black_box(vm.load(c, b"=bench"));
                }
            }
            times.push(t.elapsed().as_secs_f64() * 1e3 / ROUNDS as f64);
        }
        times.sort_by(f64::total_cmp);
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let sd = (times.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / (times.len() - 1) as f64)
            .sqrt();
        println!(
            "[load_overhead] {label}: {} chunks ({loaded} load), {bytes} bytes; \
             ms per corpus pass: median {:.3} stdev {:.3} min {:.3} max {:.3}",
            chunks.len(),
            times[SAMPLES / 2],
            sd,
            times[0],
            times[SAMPLES - 1]
        );
    }
}
