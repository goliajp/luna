//! `lua_Debug` records of function values.

use crate::runtime::{Gc, LuaClosure, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

use super::Ar;
use super::chunk_id::chunk_id;

impl Vm {
    /// PUC `lua_getinfo(">...")` on a function value.
    pub(crate) fn function_ar(&self, f: Value) -> Ar {
        match f {
            Value::Closure(cl) => self.closure_ar(cl),
            Value::Native(nc) => Ar {
                what: "C",
                source: b"=[C]".to_vec(),
                short_src: b"[C]".to_vec(),
                linedefined: -1,
                lastlinedefined: -1,
                currentline: -1,
                name: None,
                istailcall: false,
                extraargs: 0,
                ftransfer: 0,
                ntransfer: 0,
                nups: nc.upvals.len() as i64,
                nparams: 0,
                isvararg: true,
                func: f,
            },
            _ => unreachable!("a function value"),
        }
    }

    pub(crate) fn closure_ar(&self, cl: Gc<LuaClosure>) -> Ar {
        let proto = cl.proto;
        let raw = proto.source.as_bytes();
        // PUC `funcinfo` substitutes "=?" for a Proto without a source (a
        // stripped binary chunk); luna marks that as no source and no line
        // table, so a text chunk named "" still reads `[string ""]`.
        let source: Vec<u8> = if raw.is_empty() && proto.lines.is_empty() {
            b"=?".to_vec()
        } else {
            raw.to_vec()
        };
        // 5.1 functions keep their environment outside the upvalues, so
        // `_ENV` (which luna keeps in a cell) is not counted.
        let nups = if self.version() <= LuaVersion::Lua51 {
            (proto.upvals.len() - usize::from(proto.env_upval_idx != u8::MAX)) as i64
        } else {
            cl.upvals().len() as i64
        };
        Ar {
            what: if proto.line_defined == 0 {
                "main"
            } else {
                "Lua"
            },
            short_src: chunk_id(self.version(), &source),
            source,
            linedefined: proto.line_defined as i64,
            lastlinedefined: proto.last_line_defined as i64,
            currentline: -1,
            name: None,
            istailcall: false,
            extraargs: 0,
            ftransfer: 0,
            ntransfer: 0,
            nups,
            nparams: proto.num_params as i64,
            isvararg: proto.is_vararg,
            func: Value::Closure(cl),
        }
    }
}
