//! `tostring`: PUC's `luaL_tolstring` and 5.1's `luaB_tostring`, which
//! make the strings PUC makes.

use super::*;
use crate::runtime::LuaStr;
use crate::runtime::puc_fmt::FmtArg;
use crate::vm::cfmt::c_pointer;

impl Vm {
    /// `luaL_tolstring`: `__tostring` (whose result must be a string or a
    /// number, rendered), else the basic rendering, where 5.3+ names a value
    /// by a string `__name` metafield.
    pub fn tostring_value(&mut self, v: Value) -> Result<Vec<u8>, LuaError> {
        self.tostring_value_pushed(v, 0)
    }

    /// [`Vm::tostring_value`] from a native with `extra` values pushed
    /// where PUC's C function would have them: a `__tostring` it calls
    /// runs above them. Nothing is counted when there is none.
    pub(crate) fn tostring_value_pushed(
        &mut self,
        v: Value,
        extra: u32,
    ) -> Result<Vec<u8>, LuaError> {
        Ok(self.tolstring(v, extra)?.as_bytes().to_vec())
    }

    /// PUC `luaL_tolstring` (5.2+), the string made as PUC makes it.
    pub(crate) fn tolstring(&mut self, v: Value, extra: u32) -> Result<Gc<LuaStr>, LuaError> {
        let mm = self.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            // `luaL_callmeta` is a plain `lua_call`: `__tostring` cannot yield.
            self.native_push(extra);
            let r = self.call_value(mm, &[v])?;
            self.native_pop(extra);
            return match r.first().copied().unwrap_or(Value::Nil) {
                Value::Str(s) => Ok(s),
                // `lua_tolstring` turns the number into a string
                r @ (Value::Int(_) | Value::Float(_)) => self.number_string(r),
                // luaL_error over the result: positioned at whatever called
                // the library function
                _ => {
                    self.native_push_if_native(1);
                    Err(crate::vm::builtins::raise_str(
                        self,
                        "'__tostring' must return a string",
                    ))
                }
            };
        }
        let r = match v {
            Value::Str(s) => return Ok(s),
            Value::Int(_) | Value::Float(_) => {
                return match (self.version, v) {
                    // `lua_pushfstring(L, "%I")` / `"%f"`
                    (LuaVersion::Lua53 | LuaVersion::Lua54, Value::Int(i)) => {
                        self.heap.pushfstring(b"%I", &[FmtArg::I(i)])
                    }
                    (LuaVersion::Lua53 | LuaVersion::Lua54, Value::Float(f)) => {
                        self.heap.pushfstring(b"%f", &[FmtArg::F(f)])
                    }
                    _ => return self.number_string(v),
                }
                .map_err(LuaError::from);
            }
            Value::Nil => self.heap.try_intern(b"nil"),
            Value::Bool(b) => self.heap.try_intern(if b { b"true" } else { b"false" }),
            _ => {
                let kind = self.obj_typename(v);
                let p = self.topointer(v);
                self.heap
                    .pushfstring(b"%s: %p", &[FmtArg::S(kind.as_bytes()), FmtArg::P(p)])
            }
        };
        r.map_err(LuaError::from)
    }

    /// PUC 5.1 `luaB_tostring` of `v`, argument `arg` of the native at
    /// `fs`, with no `__tostring`: a number is turned into a string in its
    /// argument slot (`lua_tostring`), the string pushed found there.
    pub(crate) fn tostring_51(&mut self, v: Value, fs: u32, arg: u32) -> Result<Gc<LuaStr>, LuaError> {
        let r = match v {
            Value::Str(s) => return Ok(s),
            Value::Int(_) | Value::Float(_) => {
                let s = self.number_string(v)?;
                self.nat_set_arg(fs, arg, Value::Str(s));
                return Ok(s);
            }
            Value::Nil => self.heap.try_intern(b"nil"),
            Value::Bool(b) => self.heap.try_intern(if b { b"true" } else { b"false" }),
            _ => {
                let p = self.topointer(v);
                self.heap
                    .pushfstring(b"%s: %p", &[FmtArg::S(v.type_name().as_bytes()), FmtArg::P(p)])
            }
        };
        r.map_err(LuaError::from)
    }

    /// The string of number `v` (`lua_tolstring` converting it).
    pub(crate) fn number_string(&mut self, v: Value) -> Result<Gc<LuaStr>, LuaError> {
        let n = match v {
            Value::Int(i) => Num::Int(i),
            Value::Float(f) => Num::Float(f),
            _ => unreachable!("a number"),
        };
        let t = self.heap.number_text(n);
        self.heap.try_intern(t.as_bytes()).map_err(LuaError::from)
    }

    /// PUC `lua_topointer` of a value `tostring` names by its address.
    pub(crate) fn topointer(&self, v: Value) -> usize {
        match v {
            Value::Table(t) => t.as_ptr() as usize,
            Value::Closure(c) => c.as_ptr() as usize,
            Value::Native(n) => n.as_ptr() as usize,
            Value::Coro(co) => co.as_ptr() as usize,
            Value::Userdata(u) => self.host_userdata_ptr(u) as usize,
            Value::LightUserdata(p) => p as usize,
            _ => 0,
        }
    }

    /// The dialect's float-rendering flavor: ≤5.2 %.14g
    /// bare, 5.3/5.4 %.14g + ".0", 5.5 two-stage %.15g/%.17g + ".0".
    pub(crate) fn float_fmt(&self) -> numeric::FloatFmt {
        use crate::version::LuaVersion::*;
        match self.version {
            Lua51 | Lua52 => numeric::FloatFmt::Legacy14,
            Lua53 | Lua54 => numeric::FloatFmt::G14,
            _ => numeric::FloatFmt::TwoStage55,
        }
    }

    /// Basic tostring (no metamethods).
    pub(crate) fn tostring_basic(&mut self, v: Value) -> Vec<u8> {
        match v {
            Value::Nil => b"nil".to_vec(),
            Value::Bool(true) => b"true".to_vec(),
            Value::Bool(false) => b"false".to_vec(),
            Value::Int(i) => numeric::num_to_string(Num::Int(i)).into_bytes(),
            // PUC ≤5.2 has no integer subtype — `tostring(2.0)` is `"2"`, not
            // `"2.0"`. The 5.3+ split needs the suffix so `print(2.0)` is
            // distinguishable from `print(2)`. pm.lua :13 builds patterns by
            // concatenating these renderings.
            Value::Float(f) => {
                numeric::num_to_string_for(Num::Float(f), self.float_fmt()).into_bytes()
            }
            Value::Str(s) => s.as_bytes().to_vec(),
            Value::Table(t) => format!("table: {}", c_pointer(t.as_ptr() as usize)).into_bytes(),
            Value::Closure(c) => {
                format!("function: {}", c_pointer(c.as_ptr() as usize)).into_bytes()
            }
            Value::Native(n) => {
                format!("function: {}", c_pointer(n.as_ptr() as usize)).into_bytes()
            }
            Value::Coro(co) => format!("thread: {}", c_pointer(co.as_ptr() as usize)).into_bytes(),
            // PUC names file handles `file (0x…)`; a bare userdata is
            // `userdata: 0x…`. The io library overrides this via __tostring.
            // a userdata the C API made is known to C by its block's address
            Value::Userdata(u) => format!(
                "userdata: {}",
                c_pointer(self.host_userdata_ptr(u) as usize)
            )
            .into_bytes(),
            // PUC `lua_topointer`/tostring on light udata: "userdata: 0x…"
            // (the "light" qualifier only appears in `luaL_typeerror`).
            Value::LightUserdata(p) => format!("userdata: {}", c_pointer(p as usize)).into_bytes(),
        }
    }
}
