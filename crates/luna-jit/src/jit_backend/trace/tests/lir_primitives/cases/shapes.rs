//! The cases that are a shape of control flow, register pressure or calls
//! rather than one primitive.

use super::*;

/// Block arguments that rotate: the moves into the parameters form a cycle.
pub(super) fn swap<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let y = e.load(types::I64, mem(), p, at(1));
    let z = e.load(types::I64, mem(), p, at(2));
    let (j1, j2) = (e.create_block(), e.create_block());
    let ps1: Vec<Value> = (0..3)
        .map(|_| e.append_block_param(j1, types::I64))
        .collect();
    let ps2: Vec<Value> = (0..3)
        .map(|_| e.append_block_param(j2, types::I64))
        .collect();
    let args = |vs: &[Value]| -> Vec<BlockArg> { vs.iter().map(|&v| BlockArg::Value(v)).collect() };
    e.jump(j1, &args(&[y, z, x]));
    e.seal_block(j1);
    e.switch_to_block(j1);
    e.jump(j2, &args(&[ps1[1], ps1[2], ps1[0]]));
    e.seal_block(j2);
    e.switch_to_block(j2);
    let t = e.ishl_imm_u(ps2[1], 1);
    let r = e.isub(ps2[0], t);
    let r = e.bxor(r, ps2[2]);
    out(e, p, 0, r);
    out(e, p, 1, ps2[2]);
}

/// A loop over variables: the header reads them before its back edge is
/// known.
pub(super) fn count_loop<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let n = e.load(types::I64, mem(), p, at(2));
    let (i, acc) = (e.declare_var(types::I64), e.declare_var(types::I64));
    let zero = e.iconst(types::I64, 0);
    e.def_var(i, zero);
    e.def_var(acc, x);
    let (head, body, exit) = (e.create_block(), e.create_block(), e.create_block());
    e.jump(head, NO);
    e.switch_to_block(head);
    let iv = e.use_var(i);
    let c = e.icmp(IntCC::SignedLessThan, iv, n);
    e.brif(c, body, NO, exit, NO);
    e.seal_block(body);
    e.switch_to_block(body);
    let a = e.use_var(acc);
    let iv = e.use_var(i);
    let k = e.iconst(types::I64, 31);
    let m = e.imul(a, k);
    let t = e.bxor(iv, x);
    let a2 = e.iadd(m, t);
    e.def_var(acc, a2);
    let i2 = e.iadd_imm_s(iv, 1);
    e.def_var(i, i2);
    e.jump(head, NO);
    e.seal_block(head);
    e.seal_block(exit);
    e.switch_to_block(exit);
    let a = e.use_var(acc);
    let iv = e.use_var(i);
    out(e, p, 0, a);
    out(e, p, 1, iv);
}

/// More values live at once than either target has registers.
pub(super) fn pressure<E: Sigs>(e: &mut E, x: Value, y: Value) -> Value {
    let ints: Vec<Value> = (1..=40i64)
        .map(|k| {
            let kc = e.iconst(types::I64, k);
            let m = e.imul(x, kc);
            let a = e.iadd_imm_s(y, k);
            e.bxor(m, a)
        })
        .collect();
    let fx = e.fcvt_from_sint(types::F64, x);
    let floats: Vec<Value> = (1..=20)
        .map(|k| {
            let c = e.f64const(k as f64 * 0.75);
            e.fmul(fx, c)
        })
        .collect();
    let mut acc = e.iconst(types::I64, 0);
    for (k, &v) in ints.iter().enumerate().rev() {
        acc = if k % 2 == 0 {
            e.isub(acc, v)
        } else {
            e.bxor(acc, v)
        };
    }
    let mut facc = e.f64const(0.5);
    for &v in floats.iter().rev() {
        facc = e.fsub(v, facc);
    }
    let bits = e.bitcast(types::I64, MemFlagsData::new(), facc);
    e.iadd(acc, bits)
}

extern "C" fn mix(a: i64, b: f64, c: i64, d: f64) -> i64 {
    a.wrapping_mul(3) ^ (b.to_bits() as i64) ^ c.rotate_left(7) ^ (d.to_bits() as i64 >> 3)
}

extern "C" fn fmix(a: f64, b: f64) -> f64 {
    a * 2.0 - b
}

/// Indirect calls with integer and float arguments, values live across them.
pub(super) fn calls<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let y = e.load(types::I64, mem(), p, at(1));
    let z = e.load(types::I64, mem(), p, at(2));
    let live: Vec<Value> = (1..=16i64).map(|k| e.iadd_imm_s(x, k * 1000)).collect();
    let fy = e.fcvt_from_sint(types::F64, y);
    let mut s = e.make_sig();
    for t in [types::I64, types::F64, types::I64, types::F64] {
        s.params.push(AbiParam::new(t));
    }
    s.returns.push(AbiParam::new(types::I64));
    let s = e.sig(s);
    let mut fs = e.make_sig();
    fs.params.push(AbiParam::new(types::F64));
    fs.params.push(AbiParam::new(types::F64));
    fs.returns.push(AbiParam::new(types::F64));
    let fs = e.sig(fs);
    let callee = e.iconst(types::I64, mix as *const () as usize as i64);
    let two = e.f64const(2.5);
    let call = e.call_indirect(s, callee, &[x, fy, z, two]);
    let r1 = e.inst_results(call)[0];
    let callee = e.iconst(types::I64, fmix as *const () as usize as i64);
    let call = e.call_indirect(fs, callee, &[fy, two]);
    let r2 = e.inst_results(call)[0];
    let mut acc = r1;
    for &v in &live {
        acc = e.isub(acc, v);
        acc = e.bxor(acc, y);
    }
    out(e, p, 0, acc);
    out(e, p, 1, r2);
    out(e, p, 2, fy);
}

#[allow(clippy::too_many_arguments)]
extern "C" fn wide(
    a: i64,
    f: f64,
    b: i64,
    g: f64,
    c: i64,
    h: f64,
    d: i64,
    i: f64,
    e: i64,
    j: f64,
    k: i64,
    l: f64,
    m: i64,
    n: f64,
    o: i64,
    q: f64,
    r: i64,
) -> i64 {
    let ints = [a, b, c, d, e, k, m, o, r];
    let floats = [f, g, h, i, j, l, n, q];
    let mut acc = 0i64;
    for (s, x) in ints.iter().enumerate() {
        acc = acc.wrapping_mul(31).wrapping_add(x ^ s as i64);
    }
    for x in floats {
        acc = acc.wrapping_mul(17) ^ x.to_bits() as i64;
    }
    acc
}

/// More arguments than either target passes in registers, integers and
/// floats interleaved: the rest go on the stack in each ABI's order.
pub(super) fn many_args<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let y = e.load(types::I64, mem(), p, at(1));
    let mut s = e.make_sig();
    let mut args = Vec::new();
    for k in 0..17i64 {
        if k % 2 == 1 && k < 16 {
            s.params.push(AbiParam::new(types::F64));
            let c = e.iconst(types::I64, k);
            let v = e.iadd(y, c);
            args.push(e.fcvt_from_sint(types::F64, v));
        } else {
            s.params.push(AbiParam::new(types::I64));
            let c = e.iconst(types::I64, k * 1000);
            args.push(e.bxor(x, c));
        }
    }
    s.returns.push(AbiParam::new(types::I64));
    let s = e.sig(s);
    let callee = e.iconst(types::I64, wide as *const () as usize as i64);
    let call = e.call_indirect(s, callee, &args);
    let r = e.inst_results(call)[0];
    out(e, p, 0, r);
    // values live across the call come back intact
    let after = e.iadd(args[0], args[16]);
    out(e, p, 1, after);
}
