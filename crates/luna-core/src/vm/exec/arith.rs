//! Arithmetic opcodes: the fast paths, operand coercion per dialect and the
//! metamethod fallback.

use super::*;

impl Vm {
    /// The arithmetic opcodes' slow path: mixed operand types, string
    /// coercion, metamethods and errors. The opcode arms in the dispatch
    /// loop handle Int/Int and Float/Float themselves.
    #[inline(never)]
    pub(super) fn arith_slow(
        &mut self,
        inst: Inst,
        base: u32,
        op: ArithOp,
        l: Value,
        r: Value,
    ) -> Result<(), LuaError> {
        // An `Add` with k set is a 5.4+ `x - 0` (see `Op::Add`): the right
        // operand is the integer 0, so it adds only when the left is a number.
        let op = if inst.k() && op == ArithOp::Add && !matches!(l, Value::Int(_) | Value::Float(_))
        {
            ArithOp::Sub
        } else {
            op
        };
        match self.arith_fast(op, l, r)? {
            Some(v) => self.set_r(base, inst.a(), v),
            None => {
                let mm = self.arith_mm_func(op, l, r)?;
                let dst = base + inst.a();
                self.begin_meta_call(mm, &[l, r], MetaAction::Store { dst })?;
            }
        }
        Ok(())
    }

    /// The number a unary `-` operand stands for: 5.4+ leaves strings to
    /// the string metatable, 5.3 converts them to floats (PUC `tonumber`).
    pub(super) fn unary_operand(&self, v: Value) -> Option<Num> {
        let n = self.arith_operand()(v);
        if self.version == LuaVersion::Lua53 && matches!(v, Value::Str(_)) {
            n.map(|n| Num::Float(n.as_f64()))
        } else {
            n
        }
    }

    /// How an arithmetic operand becomes a number: 5.4+ takes numbers only
    /// (strings go to their metatable), 5.3 converts numeric strings, and
    /// 5.1/5.2, which have only floats, convert them to floats (5.1 with C
    /// `strtod`, so `"inf"` and `"0x1p4"` count).
    pub(super) fn arith_operand(&self) -> fn(Value) -> Option<Num> {
        if self.version >= LuaVersion::Lua54 {
            as_number
        } else if self.version == LuaVersion::Lua53 {
            coerce_num
        } else if self.version == LuaVersion::Lua52 {
            coerce_num_float
        } else {
            coerce_num_51
        }
    }

    /// Fast path for an arithmetic/bitwise op: `Ok(Some(v))` when computed
    /// directly, `Ok(None)` when a metamethod is required (the caller decides
    /// whether to call it synchronously or yieldably).
    pub(super) fn arith_fast(
        &mut self,
        op: ArithOp,
        l: Value,
        r: Value,
    ) -> Result<Option<Value>, LuaError> {
        use ArithOp::*;
        // 5.4 moved string->number coercion out of the VM: a string operand
        // goes to the string metatable's `__add` etc., and bitwise operators
        // have no string metamethods at all.
        let num = self.arith_operand();
        if let BAnd | BOr | BXor | Shl | Shr = op {
            let (Some(a), Some(b)) = (num(l), num(r)) else {
                return Ok(None);
            };
            let (Some(a), Some(b)) = (int_of(a), int_of(b)) else {
                // PUC luaG_tointerror: name the offending operand
                return Err(self.no_int_rep_err());
            };
            let v = match op {
                BAnd => a & b,
                BOr => a | b,
                BXor => a ^ b,
                Shl => shift_left(a, b),
                Shr => shift_left(a, b.wrapping_neg()),
                _ => unreachable!(),
            };
            return Ok(Some(Value::Int(v)));
        }
        let (Some(mut ln), Some(mut rn)) = (num(l), num(r)) else {
            return Ok(None);
        };
        // PUC 5.3 takes the integer path only when both operands are
        // integers (`ttisinteger`); a converted string goes through
        // `tonumber`, which yields a float.
        if self.version == LuaVersion::Lua53
            && (matches!(l, Value::Str(_)) || matches!(r, Value::Str(_)))
        {
            ln = Num::Float(ln.as_f64());
            rn = Num::Float(rn.as_f64());
        }
        match arith_num(self.version, op, ln, rn) {
            Ok(v) => Ok(Some(v)),
            Err(msg) => Err(self.runerror(msg)),
        }
    }

    /// Find the arithmetic/bitwise metamethod (left operand first), or raise the
    /// PUC type error when neither operand provides one.
    pub(super) fn arith_mm_func(
        &mut self,
        op: ArithOp,
        l: Value,
        r: Value,
    ) -> Result<Value, LuaError> {
        use ArithOp::*;
        let event = match op {
            Add => Mm::Add,
            Sub => Mm::Sub,
            Mul => Mm::Mul,
            Div => Mm::Div,
            Mod => Mm::Mod,
            Pow => Mm::Pow,
            IDiv => Mm::IDiv,
            BAnd => Mm::BAnd,
            BOr => Mm::BOr,
            BXor => Mm::BXor,
            Shl => Mm::Shl,
            Shr => Mm::Shr,
        };
        let mut mm = self.get_mm(l, event);
        if mm.is_nil() {
            mm = self.get_mm(r, event);
        }
        if mm.is_nil() {
            let what = if matches!(op, BAnd | BOr | BXor | Shl | Shr) {
                "perform bitwise operation on"
            } else {
                "perform arithmetic on"
            };
            // luaG_opinterror blames the first operand that is not a number;
            // before 5.4 a numeric string counts as one.
            let bad = if self.arith_operand()(l).is_none() {
                l
            } else {
                r
            };
            return Err(self.type_err(what, bad));
        }
        Ok(mm)
    }
}
