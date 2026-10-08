//! Equality, ordering and length, with their metamethod fallbacks.

use super::*;

impl Vm {
    // ---- indexing (with __index/__newindex chains) ----

    /// The `#` length operation: string byte length, `__len` if present, else
    /// the raw table border. Returns the raw length value (may be non-integer
    /// when `__len` is exotic).
    pub(crate) fn len_value(&mut self, v: Value) -> Result<Value, LuaError> {
        self.len_value_pushed(v, 0)
    }

    /// [`Vm::len_value`] with `extra` values pushed by the calling native,
    /// as [`Vm::index_value_pushed`].
    pub(crate) fn len_value_pushed(&mut self, v: Value, extra: u32) -> Result<Value, LuaError> {
        match self.len_step(v) {
            Ok(MmOut::Done(n)) => Ok(n),
            // PUC calls unary metamethods with the operand twice
            Ok(MmOut::Mm { func, recv }) => {
                self.native_push(extra);
                let r = self.call_mm1(func, &[recv, recv])?;
                self.native_pop(extra);
                Ok(r)
            }
            Ok(MmOut::CompareSynth { .. }) => unreachable!("CompareSynth from len_step"),
            Err(e) => {
                self.native_push(extra);
                Err(e)
            }
        }
    }

    /// Decide equality, or surface the `__eq` metamethod to call. `Done` carries
    /// the boolean result; `Mm` (when raw equality fails and both are tables
    /// with an `__eq`) carries the metamethod — called with `(l, r)`.
    pub(super) fn eq_step(&mut self, l: Value, r: Value) -> MmOut {
        if l.raw_eq(r) {
            return MmOut::Done(Value::Bool(true));
        }
        if let (Value::Table(_), Value::Table(_)) | (Value::Userdata(_), Value::Userdata(_)) =
            (l, r)
        {
            // PUC 5.3+ accepts any `__eq` reachable from either operand; 5.1
            // and 5.2 require the two operands' metatables to expose the same
            // `__eq` (`get_compTM` / `get_equalTM`) — `c == d` where `d` has
            // no metatable falls straight back to raw inequality. events.lua
            // 5.1 :262 bakes this in.
            let mm = if self.version() <= LuaVersion::Lua52 {
                self.get_comp_mm(l, r, Mm::Eq)
            } else {
                let mut m = self.get_mm(l, Mm::Eq);
                if m.is_nil() {
                    m = self.get_mm(r, Mm::Eq);
                }
                m
            };
            if !mm.is_nil() {
                return MmOut::Mm { func: mm, recv: l };
            }
        }
        MmOut::Done(Value::Bool(false))
    }

    // ---- arithmetic ----

    // ---- comparison ----

    /// `lua_compare(L, a, b, LUA_OPEQ)`: equality including `__eq`.
    pub(crate) fn equal(&mut self, l: Value, r: Value) -> Result<bool, LuaError> {
        self.equal_pushed(l, r, 0)
    }

    /// [`Vm::equal`] with `extra` values pushed by the calling native, as
    /// [`Vm::index_value_pushed`].
    pub(crate) fn equal_pushed(
        &mut self,
        l: Value,
        r: Value,
        extra: u32,
    ) -> Result<bool, LuaError> {
        match self.eq_step(l, r) {
            MmOut::Done(v) => Ok(v.truthy()),
            MmOut::Mm { func, .. } => {
                self.native_push(extra);
                let r = self.call_mm1(func, &[l, r])?.truthy();
                self.native_pop(extra);
                Ok(r)
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from eq_step"),
        }
    }

    pub(crate) fn less_than(&mut self, l: Value, r: Value, or_eq: bool) -> Result<bool, LuaError> {
        self.less_than_pushed(l, r, or_eq, 0)
    }

    /// [`Vm::less_than`] with `extra` values pushed by the calling native,
    /// as [`Vm::index_value_pushed`].
    pub(crate) fn less_than_pushed(
        &mut self,
        l: Value,
        r: Value,
        or_eq: bool,
        extra: u32,
    ) -> Result<bool, LuaError> {
        let step = match self.less_step(l, r, or_eq) {
            Ok(step) => step,
            Err(e) => {
                self.native_push(extra);
                return Err(e);
            }
        };
        match step {
            MmOut::Done(v) => Ok(v.truthy()),
            MmOut::Mm { func, .. } => {
                self.native_push(extra);
                let r = self.call_mm1(func, &[l, r])?.truthy();
                self.native_pop(extra);
                Ok(r)
            }
            MmOut::CompareSynth { func } => {
                // ≤5.3 `__le` via `not __lt(r, l)`. Synchronous helper used
                // by library code (sort comparator etc.) — no yield expected
                // here (a yield would have hit `call_value`'s C boundary).
                self.native_push(extra);
                let r = !self.call_mm1(func, &[r, l])?.truthy();
                self.native_pop(extra);
                Ok(r)
            }
        }
    }

    /// Decide `l < r` / `l <= r`, or surface the `__lt`/`__le` metamethod. `Done`
    /// carries the boolean result; `Mm` (for non-number/string operands) carries
    /// the metamethod — called with `(l, r)`; raises the PUC compare error when
    /// neither operand provides one.
    pub(super) fn less_step(&mut self, l: Value, r: Value, or_eq: bool) -> Result<MmOut, LuaError> {
        let b = match (l, r) {
            (Value::Int(a), Value::Int(b)) => {
                if or_eq {
                    a <= b
                } else {
                    a < b
                }
            }
            (Value::Float(a), Value::Float(b)) => {
                if or_eq {
                    a <= b
                } else {
                    a < b
                }
            }
            (Value::Int(a), Value::Float(b)) => {
                if or_eq {
                    int_le_float(a, b)
                } else {
                    int_lt_float(a, b)
                }
            }
            (Value::Float(a), Value::Int(b)) => {
                if a.is_nan() {
                    false
                } else if or_eq {
                    !int_lt_float(b, a)
                } else {
                    !int_le_float(b, a)
                }
            }
            (Value::Str(a), Value::Str(b)) => {
                let (a, b) = (a.as_bytes(), b.as_bytes());
                if or_eq { a <= b } else { a < b }
            }
            (l, r) => {
                let event = if or_eq { Mm::Le } else { Mm::Lt };
                // PUC 5.1's `get_compTM` rule applies to ordered comparisons
                // too: both operands' metatables must expose the same
                // implementation for `__lt` / `__le` to fire. events.lua 5.1
                // :262 expects `c < d` (where `d` has no metatable) to error
                // with the default "attempt to compare two table values"
                // rather than running c's `__lt` blindly.
                let mm = if self.version() <= LuaVersion::Lua51 {
                    self.get_comp_mm(l, r, event)
                } else {
                    let mut m = self.get_mm(l, event);
                    if m.is_nil() {
                        m = self.get_mm(r, event);
                    }
                    m
                };
                // PUC ≤5.4: `a <= b` falls back to `not (b < a)` when neither
                // operand carries `__le` (5.4 through its default build's
                // LUA_COMPAT_LT_LE); 5.5 requires an explicit `__le`.
                // events.lua 5.2/5.3 :172 relies on the synthesis — its
                // metatable defines only `__lt`. The `__lt` is looked up as
                // for `b < a`: on `b` first (5.1: the same one on both). The
                // fallback calls `__lt(r, l)` synchronously (the suite's
                // `__lt` doesn't yield) and negates the result; the yieldable
                // `__lt` path stays reserved for the explicit `<` operator.
                if mm.is_nil() && or_eq && self.version < LuaVersion::Lua55 {
                    let mm_lt = if self.version <= LuaVersion::Lua51 {
                        self.get_comp_mm(r, l, Mm::Lt)
                    } else {
                        let m = self.get_mm(r, Mm::Lt);
                        if m.is_nil() {
                            self.get_mm(l, Mm::Lt)
                        } else {
                            m
                        }
                    };
                    if !mm_lt.is_nil() {
                        return Ok(MmOut::CompareSynth { func: mm_lt });
                    }
                }
                if mm.is_nil() {
                    // PUC luaG_ordererror: "two X values" when the operand
                    // types match, "X with Y" otherwise (objtypename-aware).
                    let (t1, t2) = (self.obj_typename(l), self.obj_typename(r));
                    return Err(self.runerror(&if t1 == t2 {
                        format!("attempt to compare two {t1} values")
                    } else {
                        format!("attempt to compare {t1} with {t2}")
                    }));
                }
                return Ok(MmOut::Mm { func: mm, recv: l });
            }
        };
        Ok(MmOut::Done(Value::Bool(b)))
    }
}
