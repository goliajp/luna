//! 5.1's library opening counted object by object. Each `luaopen_*`
//! registers new C closures into the library table it finds
//! (`luaI_openlib`) and makes a few objects of its own; luna opens the
//! library without counting, then counts PUC's objects in PUC's order,
//! each standing on the luna value it lives as long as. The objects the
//! opening replaces die with the luna values they stand on.

use super::*;
use crate::runtime::heap::{GcHeader, PucKind};
use crate::vm::exec::host_c::HostOpenError;

/// An object of PUC's, or a change to a block of one.
#[derive(Clone, Copy)]
enum Obj {
    /// a C closure with this many upvalues
    Closure(usize),
    /// a table of this many array slots and hash nodes
    Table(usize, usize),
    /// a full userdata of this many bytes
    Udata(usize),
    /// a block of an object above resized from one size to another
    Block(usize, usize),
}

const BASE: &[&str] = &[
    "assert",
    "collectgarbage",
    "dofile",
    "error",
    "gcinfo",
    "getfenv",
    "getmetatable",
    "loadfile",
    "load",
    "loadstring",
    "next",
    "pcall",
    "print",
    "rawequal",
    "rawget",
    "rawset",
    "select",
    "setfenv",
    "setmetatable",
    "tonumber",
    "tostring",
    "type",
    "unpack",
    "xpcall",
];
const CO: &[&str] = &["create", "resume", "running", "status", "wrap", "yield"];
const TABLE: &[&str] = &[
    "concat", "foreach", "foreachi", "getn", "maxn", "insert", "remove", "setn", "sort",
];
const FILE: &[&str] = &[
    "close",
    "flush",
    "lines",
    "read",
    "seek",
    "setvbuf",
    "write",
    "__gc",
    "__tostring",
];
const IO: &[&str] = &[
    "close", "flush", "input", "lines", "open", "output", "popen", "read", "tmpfile", "type",
    "write",
];
const OS: &[&str] = &[
    "clock",
    "date",
    "difftime",
    "execute",
    "exit",
    "getenv",
    "remove",
    "rename",
    "setlocale",
    "time",
    "tmpname",
];
const STRING: &[&str] = &[
    "byte", "char", "dump", "find", "format", "gfind", "gmatch", "gsub", "len", "lower", "match",
    "rep", "reverse", "sub", "upper",
];
const MATH: &[&str] = &[
    "abs",
    "acos",
    "asin",
    "atan2",
    "atan",
    "ceil",
    "cosh",
    "cos",
    "deg",
    "exp",
    "floor",
    "fmod",
    "frexp",
    "ldexp",
    "log10",
    "log",
    "max",
    "min",
    "modf",
    "pow",
    "rad",
    "random",
    "randomseed",
    "sinh",
    "sin",
    "sqrt",
    "tanh",
    "tan",
];
const DEBUG: &[&str] = &[
    "debug",
    "getfenv",
    "gethook",
    "getinfo",
    "getlocal",
    "getregistry",
    "getmetatable",
    "getupvalue",
    "setfenv",
    "sethook",
    "setlocal",
    "setmetatable",
    "setupvalue",
    "traceback",
];

impl Vm {
    /// The objects of PUC's that library `name` holds after its opening,
    /// in the order its `luaopen_*` makes them, each with the luna value
    /// it lives as long as (`Nil` when nothing of luna's holds it: then it
    /// is garbage). `fresh`: the opening made the library table.
    fn puc_objects_51(
        &mut self,
        name: &str,
        fresh: bool,
    ) -> Result<Vec<(Value, Vec<Obj>)>, LuaError> {
        use Obj::*;
        let g = Value::Table(self.globals);
        let mut out = Vec::new();
        let lib = |vm: &mut Vm, n: &str| -> Result<Value, LuaError> { field(vm, g, n) };
        // `luaI_openlib` makes the library table when there is none
        let names = match name {
            "_G" => BASE,
            "table" => TABLE,
            "io" => IO,
            "os" => OS,
            "string" => STRING,
            "math" => MATH,
            "debug" => DEBUG,
            _ => &["loadlib", "seeall"][..],
        };
        let t = if name == "_G" { g } else { lib(self, name)? };
        if fresh && name != "_G" && name != "io" {
            out.push((t, vec![Table(0, names.len())]));
        }
        match name {
            "_G" => {
                funcs(self, &mut out, g, BASE)?;
                out.push((field(self, g, "ipairs")?, vec![Closure(0), Closure(1)]));
                out.push((field(self, g, "pairs")?, vec![Closure(0), Closure(1)]));
                // `newproxy`'s weak table, luna's metatable of the table it
                // keeps, and the function
                let np = field(self, g, "newproxy")?;
                let weak = match np {
                    Value::Native(n) => match n.upvals.first() {
                        Some(Value::Table(p)) => p.metatable().map_or(Value::Nil, Value::Table),
                        _ => Value::Nil,
                    },
                    _ => Value::Nil,
                };
                out.push((weak, vec![Table(0, 1)]));
                out.push((np, vec![Closure(1)]));
                let co = lib(self, "coroutine")?;
                if fresh {
                    out.push((co, vec![Table(0, CO.len())]));
                }
                funcs(self, &mut out, co, CO)?;
            }
            "package" => {
                let reg = self.registry.map_or(Value::Nil, Value::Table);
                out.push((field(self, reg, "_LOADLIB")?, vec![Closure(0)]));
                funcs(self, &mut out, t, names)?;
                let loaders = field(self, t, "loaders")?;
                out.push((loaders, vec![Table(4, 0)]));
                for i in 1..=4 {
                    let f = match loaders {
                        Value::Table(l) => l.get(Value::Int(i)),
                        _ => Value::Nil,
                    };
                    out.push((f, vec![Closure(0)]));
                }
                out.push((field(self, t, "preload")?, vec![Table(0, 0)]));
                funcs(self, &mut out, g, &["module", "require"])?;
            }
            "io" => {
                let mt = self.file_mt.map_or(Value::Nil, Value::Table);
                funcs(self, &mut out, mt, FILE)?;
                // the functions' environment, with its `__close`; the
                // first two default files go into it by index, and the
                // default files' own environment keeps it (its `__close`
                // was made with it as environment): it lives as long as
                // they do, which a collection finalizes before it frees
                let env = field(self, t, "stdin")?;
                out.push((env, vec![Table(0, 1), Closure(0)]));
                if fresh {
                    out.push((t, vec![Table(0, IO.len())]));
                }
                funcs(self, &mut out, t, IO)?;
                // the default files and their environment
                let s = self.heap.sizes;
                let (node, slot) = (s.node, s.tvalue);
                let stdin = field(self, t, "stdin")?;
                out.push((stdin, vec![Table(0, 1), Closure(0), Udata(s.ptr)]));
                out.push((env, vec![Block(0, slot), Block(0, node), Block(node, 0)]));
                out.push((field(self, t, "stdout")?, vec![Udata(s.ptr)]));
                out.push((
                    env,
                    vec![Block(slot, 2 * slot), Block(0, node), Block(node, 0)],
                ));
                out.push((field(self, t, "stderr")?, vec![Udata(s.ptr)]));
                // `popen`'s environment
                out.push((field(self, t, "popen")?, vec![Table(0, 1), Closure(0)]));
            }
            "string" => {
                funcs(self, &mut out, t, STRING)?;
                let mt = self.type_mt[3].map_or(Value::Nil, Value::Table);
                out.push((mt, vec![Table(0, 1)]));
            }
            _ => funcs(self, &mut out, t, names)?,
        }
        Ok(out)
    }

    /// Library `name` opened again in 5.1, counted as PUC counts it: the
    /// objects the library holds now are given the count PUC gave them
    /// (they go when the luna values they stand on go), the library opens
    /// without counting, and PUC's new objects are counted on the new luna
    /// values.
    pub(super) fn reopen_lib_51(
        &mut self,
        name: &str,
        open: Option<fn(&mut Vm) -> Result<(), LuaError>>,
    ) -> Result<(), LuaError> {
        let had = name == "_G" || self.lib_table_51(name)?.is_some();
        if had {
            let old = self.puc_objects_51(name, false)?;
            // the values that stand for nothing yet (made by the first,
            // recorded opening), before any of them gets an object
            let fresh: Vec<bool> = old
                .iter()
                .map(|(a, _)| self.stands_for_nothing(*a))
                .collect();
            for ((anchor, objs), fresh) in old.into_iter().zip(fresh) {
                if !fresh {
                    continue;
                }
                // `package`'s finalizer in `_LOADLIB` is replaced: garbage
                let anchor = match objs.as_slice() {
                    [Obj::Closure(0)] if name == "package" && matches!(anchor, Value::Table(_)) => {
                        Value::Nil
                    }
                    _ => anchor,
                };
                self.stand_51(anchor, &objs, false)?;
            }
        }
        // the C closure of `luaopen_*` the caller pushes and calls: garbage
        let c0 = self.heap.sizes.c_closure(0);
        self.heap.acct_obj(PucKind::CClosure, c0);
        self.retry_oom(|vm| vm.heap.ghost_record(PucKind::CClosure, c0))?;
        self.heap.puc.silent += 1;
        let r = self.open_lib_51(name, open);
        self.heap.puc.silent -= 1;
        r?;
        for (anchor, objs) in self.puc_objects_51(name, !had)? {
            self.stand_51(anchor, &objs, true)?;
        }
        Ok(())
    }

    /// The table `luaI_openlib` registers library `name` into: the one
    /// `package.loaded` or, failing that, the global holds.
    fn lib_table_51(&mut self, name: &str) -> Result<Option<Gc<Table>>, LuaError> {
        let loaded = self.host_loaded()?;
        let k = self.str_value(name.as_bytes())?;
        Ok(match (loaded.get(k), self.globals.get(k)) {
            (Value::Table(t), _) | (_, Value::Table(t)) => Some(t),
            _ => None,
        })
    }

    /// The library opened as its `luaopen_*` opens it, uncounted.
    fn open_lib_51(
        &mut self,
        name: &str,
        open: Option<fn(&mut Vm) -> Result<(), LuaError>>,
    ) -> Result<(), LuaError> {
        let opened = match open {
            Some(f) => self.open_lib_as(name, f),
            None => self.host_open_lib(name),
        };
        match opened {
            Ok(_) => Ok(()),
            Err(HostOpenError::Error(e)) => Err(e),
            Err(HostOpenError::Conflict(name)) => {
                let msg = format!("name conflict for module '{name}'");
                Err(LuaError(self.str_value(msg.as_bytes())?))
            }
        }
    }

    /// PUC's objects `objs` live as long as `anchor` (counted now with
    /// `count`; without, they were counted when they were made).
    fn stand_51(&mut self, anchor: Value, objs: &[Obj], count: bool) -> Result<(), LuaError> {
        let h = match anchor {
            // luna frees a table made without counting as counted, at the
            // size PUC's of the same shape has: nothing to stand on it
            Value::Table(_) => {
                if count {
                    self.count_51(objs);
                }
                return Ok(());
            }
            Value::Native(n) => n.as_ptr() as *mut GcHeader,
            Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
            _ => std::ptr::null_mut(),
        };
        let s = self.heap.sizes;
        let mut sizes = Vec::new();
        for &o in objs {
            match o {
                Obj::Closure(n) => {
                    let c = s.c_closure(n);
                    if count {
                        self.heap.acct_obj(PucKind::CClosure, c);
                    }
                    sizes.push(c);
                }
                Obj::Table(a, n) => {
                    let (ab, hb) = (self.heap.array_bytes_of(a), self.heap.hash_bytes_of(n));
                    if count {
                        self.heap.acct_obj(PucKind::Table, s.table);
                        self.heap.acct_block(0, ab);
                        self.heap.acct_block(0, hb);
                    }
                    sizes.extend([hb, ab, s.table].into_iter().filter(|&b| b > 0));
                }
                Obj::Udata(b) => {
                    let u = s.userdata(1, b);
                    if count {
                        self.heap.acct_obj(PucKind::Userdata, u);
                    }
                    sizes.push(u);
                }
                Obj::Block(from, to) => {
                    if count {
                        self.heap.acct_block(from, to);
                    }
                    if to > from {
                        sizes.push(to - from);
                    } else if let Some(i) = sizes.iter().rposition(|&x| x == from - to) {
                        sizes.remove(i);
                    }
                }
            }
        }
        if h.is_null() {
            // nothing of luna's holds it: garbage at once (a new object of
            // PUC's, or an old one the opening replaces)
            for b in sizes {
                self.retry_oom(|vm| vm.heap.ghost_record(PucKind::Table, b))?;
            }
            return Ok(());
        }
        self.heap.stand_sizes_on(h, sizes);
        Ok(())
    }

    /// Whether `v` is a value of luna's made without counting that no
    /// object of PUC's stands on yet.
    fn stands_for_nothing(&self, v: Value) -> bool {
        let h = match v {
            Value::Native(n) => n.as_ptr() as *mut GcHeader,
            Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
            Value::Table(_) => return true,
            _ => return true,
        };
        crate::runtime::heap::is_uncounted(h) && !self.heap.stands(h)
    }
}

impl Vm {
    /// PUC's objects `objs` counted, made now.
    fn count_51(&mut self, objs: &[Obj]) {
        let s = self.heap.sizes;
        for &o in objs {
            match o {
                Obj::Closure(n) => self.heap.acct_obj(PucKind::CClosure, s.c_closure(n)),
                Obj::Table(a, n) => {
                    self.heap.acct_obj(PucKind::Table, s.table);
                    let (ab, hb) = (self.heap.array_bytes_of(a), self.heap.hash_bytes_of(n));
                    self.heap.acct_block(0, ab);
                    self.heap.acct_block(0, hb);
                }
                Obj::Udata(b) => self.heap.acct_obj(PucKind::Userdata, s.userdata(1, b)),
                Obj::Block(from, to) => self.heap.acct_block(from, to),
            }
        }
    }
}

/// One C closure each for the functions `names` of table `t`.
fn funcs(
    vm: &mut Vm,
    out: &mut Vec<(Value, Vec<Obj>)>,
    t: Value,
    names: &[&str],
) -> Result<(), LuaError> {
    for &n in names {
        out.push((field(vm, t, n)?, vec![Obj::Closure(0)]));
    }
    Ok(())
}

/// Field `k` of table `t` (nil when `t` is not a table).
fn field(vm: &mut Vm, t: Value, k: &str) -> Result<Value, LuaError> {
    let Value::Table(t) = t else {
        return Ok(Value::Nil);
    };
    let k = vm.str_value(k.as_bytes())?;
    Ok(t.get(k))
}
