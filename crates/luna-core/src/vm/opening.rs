//! Opening the standard libraries as PUC's `luaL_openlibs` opens them: a
//! script of the C API and auxiliary library calls PUC makes, in its
//! order (`opening/data.rs`, generated from PUC), run on luna's stack with
//! luna's objects. Each call allocates, counts and checks for a
//! collection as PUC's does, so the collector steps where PUC's steps.
//! The C functions are luna's natives, from each library's table of PUC's
//! functions (`MODULES`).

use crate::runtime::{Builtin, Value};
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

#[allow(clippy::all)]
#[rustfmt::skip]
mod data;
mod api;
mod fenv51;
mod lauxlib;
mod run;
mod step;

pub(crate) use run::{Ix, Opener};

/// What PUC's `luaL_openlibs` does in one dialect (see `data.rs`): flat,
/// each part found by where it ends, so the loader has no pointers to
/// relocate.
pub(crate) struct Script {
    /// the calls of the top level of `luaL_openlibs`, then of each
    /// `luaopen_*`
    pub(crate) ops: &'static [u8],
    pub(crate) top_end: usize,
    /// each `luaopen_*`'s number and where its calls end
    pub(crate) opens: &'static [(u32, u32)],
    pub(crate) strs: &'static [u8],
    pub(crate) str_ends: &'static [u16],
    pub(crate) nums: &'static [f64],
    /// `luaL_Reg` lists: a name's string index, the function (0: none)
    pub(crate) items: &'static [(u16, u32)],
    pub(crate) list_ends: &'static [u16],
}

impl Script {
    /// String `i` (from 0).
    #[inline]
    pub(crate) fn str(&self, i: usize) -> &'static [u8] {
        let start = i
            .checked_sub(1)
            .map_or(0, |j| usize::from(self.str_ends[j]));
        &self.strs[start..usize::from(self.str_ends[i])]
    }

    /// List `i`.
    pub(crate) fn list(&self, i: usize) -> &'static [(u16, u32)] {
        let start = i
            .checked_sub(1)
            .map_or(0, |j| usize::from(self.list_ends[j]));
        &self.items[start..usize::from(self.list_ends[i])]
    }

    /// The calls of the `luaopen_*` the script numbers `f` (0-based), or
    /// none.
    fn segment(&self, found: impl Fn(u32) -> bool) -> &'static [u8] {
        let mut start = self.top_end;
        for &(f, end) in self.opens {
            if found(f) {
                return &self.ops[start..end as usize];
            }
            start = end as usize;
        }
        &[]
    }
}

/// How luna makes the function PUC's script names.
#[derive(Clone, Copy)]
pub(crate) enum Ctor {
    /// a library function with no upvalues of luna's own: from 5.2 on a
    /// light C function when PUC's has none, a C closure of PUC's
    /// upvalues otherwise
    F(crate::runtime::value::NativeFn, Builtin),
    /// made by its own code, with PUC's upvalues the top `nup` values of
    /// the opener's stack
    C(fn(&mut Vm, &mut Opener, u32, u32) -> Result<Value, LuaError>),
    /// a library's `luaopen_*`, a native that runs its part of the script
    /// (see [`run_open`])
    Open(crate::runtime::value::NativeFn),
}

/// A table of PUC's C functions: its name in PUC, and how luna makes it.
pub(crate) type Natives = &'static [(&'static str, Ctor)];

/// Every table, in the order the generator numbers them: a function is
/// `module << 8 | index`.
const TABLES: [Natives; 11] = [
    crate::vm::builtins::PUC_FNS,
    crate::vm::lib_coroutine::PUC_FNS,
    crate::vm::lib_string::PUC_FNS,
    crate::vm::lib_table::PUC_FNS,
    crate::vm::lib_math::PUC_FNS,
    crate::vm::lib_io::PUC_FNS,
    crate::vm::lib_os_io::PUC_FNS,
    crate::vm::lib_package::PUC_FNS,
    crate::vm::lib_debug::PUC_FNS,
    crate::vm::lib_bit32::PUC_FNS,
    crate::vm::lib_utf8::PUC_FNS,
];

/// [`TABLES`] in one place: a `const` is copied wherever it is used.
static MODULES: [Natives; 11] = TABLES;

/// Where each table's functions start in a numbering of them all.
const OFFSETS: [u16; 12] = {
    let mut o = [0u16; 12];
    let mut i = 0;
    while i < TABLES.len() {
        o[i + 1] = o[i] + TABLES[i].len() as u16;
        i += 1;
    }
    o
};

/// How many functions the tables have.
pub(crate) const FN_COUNT: usize = OFFSETS[11] as usize;

/// The function the script numbers `f` (1 + `module << 8 | index`), and
/// its place in a numbering of all of them.
#[inline]
pub(crate) fn native(f: u32) -> (Ctor, usize) {
    let f = f - 1;
    let (m, i) = ((f >> 8) as usize, (f & 0xFF) as usize);
    (MODULES[m][i].1, usize::from(OFFSETS[m]) + i)
}

/// The number the script gives the C function PUC names `name`.
pub(crate) fn fn_number(name: &str) -> u32 {
    for (m, fns) in MODULES.iter().enumerate() {
        if let Some(i) = fns.iter().position(|&(n, _)| n == name) {
            return 1 + ((m as u32) << 8 | i as u32);
        }
    }
    unreachable!("a function of the tables")
}

/// The script of the dialect `v`.
fn script(v: LuaVersion) -> &'static Script {
    match v {
        LuaVersion::Lua51 => &data::SCRIPT_51,
        LuaVersion::Lua52 => &data::SCRIPT_52,
        LuaVersion::Lua53 => &data::SCRIPT_53,
        LuaVersion::Lua55 => &data::SCRIPT_55,
        _ => &data::SCRIPT_54,
    }
}

/// The body of each library's `luaopen_*` (`Ctor::Open`), `name` in
/// PUC: the running native runs its part of the script, and returns the
/// `nret` values it left on top.
pub(crate) fn run_open(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    name: &str,
    nret: u32,
) -> Result<u32, LuaError> {
    let s = script(vm.version());
    // by name: the compiler may merge the bodies of the openers
    let ops = s.segment(|f| MODULES[(f >> 8) as usize][(f & 0xFF) as usize].0 == name);
    // room for what the calls push (PUC's C function has `LUA_MINSTACK`
    // slots), so a push only writes its slot
    vm.grow_stack(fs as usize + 1 + nargs as usize + 24)?;
    let mut op = Opener::native(vm, s, fs);
    op.sandbox = vm.opening_sandbox;
    op.run(vm, ops)?;
    let vals = op.top_values(vm, nret);
    vm.nat_ret(fs, &vals)
}

/// The script's number of the `luaopen_*` of library `name` (`_G` for
/// the base library), if the dialect has the library.
fn opener_of(s: &Script, name: &str) -> Option<u32> {
    let lib = if name == "_G" { "base" } else { name };
    s.opens.iter().map(|&(f, _)| f + 1).find(|&f| {
        let f = f - 1;
        let n = MODULES[(f >> 8) as usize][(f & 0xFF) as usize].0;
        n.strip_prefix("luaopen_") == Some(lib)
    })
}

impl Vm {
    /// PUC's `luaL_openlibs` (`luaL_openselectedlibs` in 5.5) at the host
    /// level: the script's top level.
    pub(crate) fn run_openlibs(&mut self) -> Result<(), LuaError> {
        let s = script(self.version());
        let mut op = Opener::host(self, s);
        let r = op.run(self, &s.ops[..s.top_end]);
        op.leave(self);
        r
    }

    /// Library `name` (`_G` for the base library) opened at the host level
    /// as PUC opens it one at a time: `luaL_requiref(L, name, luaopen_*,
    /// 1)`, in 5.1 a call of `luaopen_*` with the name.
    pub(crate) fn run_open_one(&mut self, name: &str) -> Result<(), LuaError> {
        let s = script(self.version());
        // `bit32` where PUC has none: luna's own library
        if name == "bit32" && opener_of(s, name).is_none() {
            return crate::vm::lib_bit32::try_open_bit32(self);
        }
        let mut op = Opener::host(self, s);
        let r = op.open_one(self, name);
        op.leave(self);
        r
    }

    /// The body of library `name`'s `luaopen_*` run at the host level, as
    /// the C API's `luaopen_*` runs it: what it returns.
    pub(crate) fn run_open_body(&mut self, name: &str) -> Result<Vec<Value>, LuaError> {
        let s = script(self.version());
        let Some(f) = opener_of(s, name) else {
            if name == "bit32" {
                crate::vm::lib_bit32::try_open_bit32(self)?;
                let g = self.globals();
                let lib = self
                    .heap
                    .find_interned(b"bit32")
                    .map(|k| g.get(Value::Str(k)));
                return Ok(vec![lib.unwrap_or(Value::Nil)]);
            }
            return Ok(vec![Value::Nil]);
        };
        let ops = s.segment(|g| g + 1 == f);
        let mut op = Opener::host(self, s);
        let r = op.run(self, ops);
        let n = if self.version() == LuaVersion::Lua51 && name == "_G" {
            2
        } else {
            1
        };
        let vals = op.top_values(self, n);
        op.leave(self);
        r.map(|()| vals)
    }
}
