//! Naming the operand of a runtime error (PUC `varinfo`).

use super::*;

impl Vm {
    /// Name the offending operand of the current instruction (PUC varinfo) for
    /// a type error, e.g. " (global 'x')". The faulting value `bad` is matched
    /// to the instruction's subject register(s); a native-raised error whose
    /// current instruction doesn't hold `bad` simply yields "".
    pub(crate) fn subject_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
        // PUC `varinfo` names a variable only for a Lua activation
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.last().and_then(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        let mut cands: Vec<u32> = Vec::new();
        match instr.op() {
            // indexed reads / length / method: the table/object is in B
            Op::GetField | Op::GetI | Op::GetTable | Op::GetTableK | Op::SelfOp | Op::Len => {
                cands.push(instr.b());
            }
            // indexed writes / calls: the table/function is in A
            Op::SetField | Op::SetI | Op::SetTable | Op::SetTableK | Op::Call | Op::TailCall => {
                cands.push(instr.a());
            }
            // arithmetic/bitwise: a register operand (B, and C unless constant)
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Mod
            | Op::Pow
            | Op::IDiv
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr => {
                cands.push(instr.b());
                if !instr.k() {
                    cands.push(instr.c());
                }
            }
            Op::Unm | Op::BNot => cands.push(instr.b()),
            // arithmetic on a constant or an immediate: the register operand
            Op::AddI
            | Op::SubI
            | Op::AddK
            | Op::SubK
            | Op::MulK
            | Op::ModK
            | Op::PowK
            | Op::DivK
            | Op::IDivK
            | Op::BAndK
            | Op::BOrK
            | Op::BXorK
            | Op::ShrI
            | Op::ShlI => cands.push(instr.b()),
            // indexing an upvalue table (`_ENV` for a global): PUC
            // `getupvalname` finds the value among the closure's upvalues
            Op::GetTabUp | Op::GetTabUpR | Op::SetTabUp | Op::SetTabUpR | Op::SetTabUpK => {
                let u = if matches!(instr.op(), Op::GetTabUp | Op::GetTabUpR) {
                    instr.b()
                } else {
                    instr.a()
                };
                if self.upval_get(f.closure, u).raw_eq(bad)
                    && let Some(d) = p.upvals.get(u as usize)
                {
                    return format!(" (upvalue '{}')", d.name);
                }
            }
            Op::Concat => {
                let a = instr.a();
                for r in a..a + instr.b() {
                    cands.push(r);
                }
            }
            _ => {}
        }
        // Up to 5.3 a binary operator takes a constant operand straight
        // from the constant table (RK), where `varinfo` cannot see it, so a
        // string constant is not named there; unary operators load it into
        // a register first and do name it.
        let rk_operands = self.version <= LuaVersion::Lua53
            && matches!(
                instr.source_op(),
                Op::Add
                    | Op::Sub
                    | Op::Mul
                    | Op::Div
                    | Op::Mod
                    | Op::Pow
                    | Op::IDiv
                    | Op::BAnd
                    | Op::BOr
                    | Op::BXor
                    | Op::Shl
                    | Op::Shr
            );
        for reg in cands {
            if self.r(f.base, reg).raw_eq(bad) {
                return match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                    Some(("constant", _)) if rk_operands => String::new(),
                    Some((kind, name)) => format!(" ({kind} '{name}')"),
                    None => String::new(),
                };
            }
        }
        String::new()
    }

    /// "attempt to call a X value", enriched (PUC luaG_callerror) with a name
    /// for the call target: "(global 'f')" for a direct call, or "(metamethod
    /// 'add')" when the call is a metamethod dispatched by the current opcode.
    pub(crate) fn call_err(&mut self, v: Value) -> LuaError {
        let extra = self.call_target_varinfo(v);
        let tn = self.obj_typename(v);
        let msg = self.compose_type_err("call", &tn, &extra);
        self.runerror_named(&msg, &extra)
    }

    /// Name the offending call target. A metamethod dispatch pushes a `Cont`
    /// frame before the call, so the opcode that triggered it lives in the
    /// nearest *Lua* frame — read that instruction: OP_CALL names the function
    /// register, any metamethod-bearing opcode yields "(metamethod 'event')".
    pub(crate) fn call_target_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
        // a hook's call (PUC `funcnamefromcall` on a `CIST_HOOKED` caller)
        if self.pending_is_hook && self.version >= LuaVersion::Lua54 {
            return " (hook '?')".to_string();
        }
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.iter().rev().find_map(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        match instr.source_op() {
            Op::Call | Op::TailCall => {
                let reg = instr.a();
                if self.r(f.base, reg).raw_eq(bad) {
                    match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                        Some((kind, name)) => format!(" ({kind} '{name}')"),
                        None => String::new(),
                    }
                } else {
                    String::new()
                }
            }
            // 5.4 `funcnamefromcode` names the generic-for iterator call
            // (5.3 had the entry but raised through plain `luaG_typeerror`)
            Op::TForCall if self.version >= LuaVersion::Lua54 => {
                " (for iterator 'for iterator')".to_string()
            }
            // 5.4 `funcnamefromcall` names the metamethod; up to 5.3 the
            // call raised through `luaG_typeerror`, whose `varinfo` does not
            op if self.version >= LuaVersion::Lua54 => match mm_event_name(op) {
                Some(ev) => format!(" (metamethod '{ev}')"),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    /// "number has no integer representation", enriched (PUC luaG_tointerror)
    /// with a "(field 'x')"-style suffix naming the offending operand of the
    /// current arithmetic instruction when it can be recovered from bytecode.
    pub(crate) fn no_int_rep_err(&mut self) -> LuaError {
        let extra = self.bad_operand_varinfo();
        self.runerror_named(
            &format!("number{extra} has no integer representation"),
            &extra,
        )
    }

    /// Inspect the current frame's faulting instruction: find the register
    /// operand holding a float with no integer representation and name it.
    pub(crate) fn bad_operand_varinfo(&self) -> String {
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.last().and_then(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        let mut regs = vec![instr.b()];
        // C of a constant- or immediate-operand opcode is not a register
        if !instr.k() && instr.arith_const_op().is_none() {
            regs.push(instr.c());
        }
        let no_int = |n: Option<Num>| matches!(n, Some(Num::Float(x)) if crate::runtime::value::f2i_exact(x).is_none());
        for reg in regs {
            let v = self.r(f.base, reg);
            // before 5.4 a numeric string is converted first, so "2.5" is
            // the operand without an integer value
            let n = self.arith_operand()(v);
            if no_int(n) {
                return match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                    Some((kind, name)) => format!(" ({kind} '{name}')"),
                    None => String::new(),
                };
            }
        }
        String::new()
    }
}
