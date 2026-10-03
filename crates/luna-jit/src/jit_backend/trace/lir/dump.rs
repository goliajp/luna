//! `LUNA_BASELINE_DUMP=<dir>` writes each baseline trace's instructions,
//! their register assignment (`trace-N.txt`) and its machine code
//! (`trace-N.bin`, for a disassembler) into `<dir>`.

use super::alloc::{Allocation, Loc};
use super::live::Analysis;
use super::*;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

pub(crate) fn dir() -> Option<std::path::PathBuf> {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("LUNA_BASELINE_DUMP").map(Into::into))
        .clone()
}

fn loc(l: Loc) -> String {
    match l {
        Loc::None => "-".into(),
        Loc::Reg(r) => format!("r{r}"),
        Loc::Stack(s) => format!("[s{s}]"),
    }
}

pub(crate) fn write(lir: &Lir, an: &Analysis, al: &Allocation, code: &[u8]) {
    let Some(dir) = dir() else { return };
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let nv = an.n_values;
    let name = |r: u32| -> String {
        if r == NONE {
            return "_".into();
        }
        let l = loc(al.loc[r as usize]);
        if r < nv {
            format!("v{r}:{l}")
        } else {
            format!("var{}:{l}", r - nv)
        }
    };
    let mut s = String::new();
    for &b in &an.order {
        let params: Vec<String> = lir.block_params(b).iter().map(|&p| name(p)).collect();
        let _ = writeln!(s, "block{b}({}):", params.join(", "));
        let (lo, hi) = an.block_at[b as usize];
        for c in lo..hi {
            let ii = an.code[c as usize];
            let i = &lir.insts[ii as usize];
            let args: Vec<String> = lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize]
                .iter()
                .map(|&a| name(a))
                .collect();
            let (a, b2, c2) = match i.op {
                Op::VarRead | Op::VarWrite => (format!("var{}", i.a), name(i.b), String::new()),
                Op::Jump => (format!("block{}", i.a), String::new(), String::new()),
                Op::Brif(_) => (name(i.a), format!("block{}", i.b), format!("block{}", i.c)),
                Op::Call => (format!("fn{}", i.a), String::new(), String::new()),
                Op::Load(_) | Op::Uload8(_) | Op::Store(_) => {
                    (name(i.a), name(i.b), format!("trusted={}", i.c))
                }
                _ => (name(i.a), name(i.b), name(i.c)),
            };
            let _ = writeln!(
                s,
                "  @{} {} = {:?}.{:?} {a} {b2} {c2} [{}]",
                2 * c,
                name(i.dst),
                i.op,
                i.ty,
                args.join(", ")
            );
        }
    }
    let _ = std::fs::write(dir.join(format!("trace-{n}.txt")), s);
    let _ = std::fs::write(dir.join(format!("trace-{n}.bin")), code);
}
