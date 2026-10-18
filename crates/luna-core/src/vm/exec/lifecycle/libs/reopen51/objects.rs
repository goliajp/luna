//! What 5.1's libraries are made of: the objects of PUC's each
//! `luaopen_*` makes, in its order, with the luna values they stand on.

use super::*;

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
    pub(super) fn puc_objects_51(
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
