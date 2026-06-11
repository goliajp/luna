//! Function objects: compiled prototypes, Lua closures, upvalues.

use crate::runtime::heap::{Gc, GcHeader, Marker};
use crate::runtime::string::LuaStr;
use crate::runtime::value::Value;
use crate::vm::isa::Inst;

/// Where a closure's upvalue is captured from, relative to the *enclosing*
/// function (PUC Upvaldesc).
#[derive(Clone, Debug)]
pub struct UpvalDesc {
    /// captured from the enclosing frame's registers (true) or from the
    /// enclosing closure's own upvalues (false)
    pub in_stack: bool,
    pub index: u8,
    /// variable name, for error messages and debug info
    pub name: Box<str>,
}

/// A compiled function (PUC Proto). Immutable after compilation.
#[repr(C)]
pub struct Proto {
    pub(crate) hdr: GcHeader,
    pub code: Box<[Inst]>,
    pub consts: Box<[Value]>,
    pub protos: Box<[Gc<Proto>]>,
    pub upvals: Box<[UpvalDesc]>,
    pub num_params: u8,
    pub is_vararg: bool,
    /// registers needed by a frame of this function
    pub max_stack: u8,
    /// line of each instruction (same length as `code`)
    pub lines: Box<[u32]>,
    /// chunk name, for error messages
    pub source: Gc<LuaStr>,
    pub line_defined: u32,
}

impl Proto {
    pub(crate) fn trace(&self, m: &mut Marker) {
        for &k in self.consts.iter() {
            m.value(k);
        }
        for &p in self.protos.iter() {
            m.header(p.as_ptr() as *mut GcHeader);
        }
        m.header(self.source.as_ptr() as *mut GcHeader);
    }
}

#[repr(C)]
pub struct LuaClosure {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    pub proto: Gc<Proto>,
    pub upvals: Box<[Gc<Upvalue>]>,
}

impl LuaClosure {
    pub(crate) fn trace(&self, m: &mut Marker) {
        m.header(self.proto.as_ptr() as *mut GcHeader);
        for &uv in self.upvals.iter() {
            m.header(uv.as_ptr() as *mut GcHeader);
        }
    }
}

/// An upvalue cell. Open: refers to a live VM stack slot (the stack is a GC
/// root, so open cells trace nothing). Closed: owns the value inline.
#[repr(C)]
pub struct Upvalue {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    pub(crate) state: UpvalState,
}

#[derive(Clone, Copy)]
pub enum UpvalState {
    Open(u32),
    Closed(Value),
}

impl Upvalue {
    pub fn state(&self) -> UpvalState {
        self.state
    }

    pub(crate) fn trace(&self, m: &mut Marker) {
        if let UpvalState::Closed(v) = self.state {
            m.value(v);
        }
    }
}
