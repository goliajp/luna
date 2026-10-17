// CODEGEN: gctrace libgen.sh (an instrumented PUC's luaL_openlibs)
//! What PUC's `luaL_openlibs` does to the parts of each library
//! table it names, by dialect, from the table's making on: its
//! resizes other than a rehash's and the string keys it puts in, in
//! order. Where a key lands in the hash part (and so the order
//! `next` gives) depends on it.

use super::LibOp::{self, K, R};

pub(crate) static LAYOUT_51: &[(&str, &[LibOp])] = &[
    ("FILE*", &[K("__index"), K("close"), K("flush"), K("lines"), K("read"), K("seek"), K("setvbuf"), K("write"), K("__gc"), K("__tostring")]),
    ("_G", &[R(0, 2), K("_G"), K("assert"), K("collectgarbage"), K("dofile"), K("error"), K("gcinfo"), K("getfenv"), K("getmetatable"), K("loadfile"), K("load"), K("loadstring"), K("next"), K("pcall"), K("print"), K("rawequal"), K("rawget"), K("rawset"), K("select"), K("setfenv"), K("setmetatable"), K("tonumber"), K("tostring"), K("type"), K("unpack"), K("xpcall"), K("_VERSION"), K("ipairs"), K("pairs"), K("newproxy"), K("coroutine"), K("package"), K("module"), K("require"), K("table"), K("io"), K("os"), K("string"), K("math"), K("debug")]),
    ("coroutine", &[R(0, 6), K("create"), K("resume"), K("running"), K("status"), K("wrap"), K("yield")]),
    ("debug", &[R(0, 14), K("debug"), K("getfenv"), K("gethook"), K("getinfo"), K("getlocal"), K("getregistry"), K("getmetatable"), K("getupvalue"), K("setfenv"), K("sethook"), K("setlocal"), K("setmetatable"), K("setupvalue"), K("traceback")]),
    ("io", &[R(0, 11), K("close"), K("flush"), K("input"), K("lines"), K("open"), K("output"), K("popen"), K("read"), K("tmpfile"), K("type"), K("write"), K("stdin"), K("stdout"), K("stderr")]),
    ("math", &[R(0, 28), K("abs"), K("acos"), K("asin"), K("atan2"), K("atan"), K("ceil"), K("cosh"), K("cos"), K("deg"), K("exp"), K("floor"), K("fmod"), K("frexp"), K("ldexp"), K("log10"), K("log"), K("max"), K("min"), K("modf"), K("pow"), K("rad"), K("random"), K("randomseed"), K("sinh"), K("sin"), K("sqrt"), K("tanh"), K("tan"), K("pi"), K("huge"), K("mod")]),
    ("os", &[R(0, 11), K("clock"), K("date"), K("difftime"), K("execute"), K("exit"), K("getenv"), K("remove"), K("rename"), K("setlocale"), K("time"), K("tmpname")]),
    ("package", &[R(0, 2), K("loadlib"), K("seeall"), K("loaders"), K("path"), K("cpath"), K("config"), K("loaded"), K("preload")]),
    ("package.loaders", &[R(4, 0)]),
    ("registry", &[R(0, 2), K("_LOADED"), K("_LOADLIB"), K("FILE*")]),
    ("registry._LOADED", &[R(0, 1), K("_G"), K("coroutine"), K("package"), K("table"), K("io"), K("os"), K("string"), K("math"), K("debug")]),
    ("registry._LOADLIB", &[K("__gc")]),
    ("string", &[R(0, 15), K("byte"), K("char"), K("dump"), K("find"), K("format"), K("gfind"), K("gmatch"), K("gsub"), K("len"), K("lower"), K("match"), K("rep"), K("reverse"), K("sub"), K("upper")]),
    ("string_mt", &[R(0, 1), K("__index")]),
    ("table", &[R(0, 9), K("concat"), K("foreach"), K("foreachi"), K("getn"), K("maxn"), K("insert"), K("remove"), K("setn"), K("sort")]),
];

pub(crate) static LAYOUT_52: &[(&str, &[LibOp])] = &[
    ("FILE*", &[K("__index"), K("close"), K("flush"), K("lines"), K("read"), K("seek"), K("setvbuf"), K("write"), K("__gc"), K("__tostring")]),
    ("_G", &[K("_G"), K("assert"), K("collectgarbage"), K("dofile"), K("error"), K("getmetatable"), K("ipairs"), K("loadfile"), K("load"), K("loadstring"), K("next"), K("pairs"), K("pcall"), K("print"), K("rawequal"), K("rawlen"), K("rawget"), K("rawset"), K("select"), K("setmetatable"), K("tonumber"), K("tostring"), K("type"), K("xpcall"), K("_VERSION"), K("module"), K("require"), K("package"), K("coroutine"), K("unpack"), K("table"), K("io"), K("os"), K("string"), K("bit32"), K("math"), K("debug")]),
    ("bit32", &[R(0, 12), K("arshift"), K("band"), K("bnot"), K("bor"), K("bxor"), K("btest"), K("extract"), K("lrotate"), K("lshift"), K("replace"), K("rrotate"), K("rshift")]),
    ("coroutine", &[R(0, 6), K("create"), K("resume"), K("running"), K("status"), K("wrap"), K("yield")]),
    ("debug", &[R(0, 16), K("debug"), K("getuservalue"), K("gethook"), K("getinfo"), K("getlocal"), K("getregistry"), K("getmetatable"), K("getupvalue"), K("upvaluejoin"), K("upvalueid"), K("setuservalue"), K("sethook"), K("setlocal"), K("setmetatable"), K("setupvalue"), K("traceback")]),
    ("io", &[R(0, 11), K("close"), K("flush"), K("input"), K("lines"), K("open"), K("output"), K("popen"), K("read"), K("tmpfile"), K("type"), K("write"), K("stdin"), K("stdout"), K("stderr")]),
    ("math", &[R(0, 28), K("abs"), K("acos"), K("asin"), K("atan2"), K("atan"), K("ceil"), K("cosh"), K("cos"), K("deg"), K("exp"), K("floor"), K("fmod"), K("frexp"), K("ldexp"), K("log10"), K("log"), K("max"), K("min"), K("modf"), K("pow"), K("rad"), K("random"), K("randomseed"), K("sinh"), K("sin"), K("sqrt"), K("tanh"), K("tan"), K("pi"), K("huge")]),
    ("os", &[R(0, 11), K("clock"), K("date"), K("difftime"), K("execute"), K("exit"), K("getenv"), K("remove"), K("rename"), K("setlocale"), K("time"), K("tmpname")]),
    ("package", &[R(0, 3), K("loadlib"), K("searchpath"), K("seeall"), K("loaders"), K("searchers"), K("path"), K("cpath"), K("config"), K("loaded"), K("preload")]),
    ("package.loaders", &[R(4, 0)]),
    ("registry", &[R(2, 0), K("_LOADED"), K("_CLIBS"), K("_PRELOAD"), K("FILE*"), K("_IO_input"), K("_IO_output")]),
    ("registry._LOADED", &[K("_G"), K("package"), K("coroutine"), K("table"), K("io"), K("os"), K("string"), K("bit32"), K("math"), K("debug")]),
    ("string", &[R(0, 14), K("byte"), K("char"), K("dump"), K("find"), K("format"), K("gmatch"), K("gsub"), K("len"), K("lower"), K("match"), K("rep"), K("reverse"), K("sub"), K("upper")]),
    ("string_mt", &[R(0, 1), K("__index")]),
    ("table", &[R(0, 7), K("concat"), K("maxn"), K("insert"), K("pack"), K("unpack"), K("remove"), K("sort")]),
];

pub(crate) static LAYOUT_53: &[(&str, &[LibOp])] = &[
    ("FILE*", &[R(0, 2), K("__name"), K("__index"), K("close"), K("flush"), K("lines"), K("read"), K("seek"), K("setvbuf"), K("write"), K("__gc"), K("__tostring")]),
    ("_G", &[K("assert"), K("collectgarbage"), K("dofile"), K("error"), K("getmetatable"), K("ipairs"), K("loadfile"), K("load"), K("next"), K("pairs"), K("pcall"), K("print"), K("rawequal"), K("rawlen"), K("rawget"), K("rawset"), K("select"), K("setmetatable"), K("tonumber"), K("tostring"), K("type"), K("xpcall"), K("_G"), K("_VERSION"), K("require"), K("package"), K("coroutine"), K("table"), K("io"), K("os"), K("string"), K("math"), K("utf8"), K("debug"), K("bit32")]),
    ("bit32", &[R(0, 12), K("arshift"), K("band"), K("bnot"), K("bor"), K("bxor"), K("btest"), K("extract"), K("lrotate"), K("lshift"), K("replace"), K("rrotate"), K("rshift")]),
    ("coroutine", &[R(0, 7), K("create"), K("resume"), K("running"), K("status"), K("wrap"), K("yield"), K("isyieldable")]),
    ("debug", &[R(0, 16), K("debug"), K("getuservalue"), K("gethook"), K("getinfo"), K("getlocal"), K("getregistry"), K("getmetatable"), K("getupvalue"), K("upvaluejoin"), K("upvalueid"), K("setuservalue"), K("sethook"), K("setlocal"), K("setmetatable"), K("setupvalue"), K("traceback")]),
    ("io", &[R(0, 11), K("close"), K("flush"), K("input"), K("lines"), K("open"), K("output"), K("popen"), K("read"), K("tmpfile"), K("type"), K("write"), K("stdin"), K("stdout"), K("stderr")]),
    ("math", &[R(0, 35), K("abs"), K("acos"), K("asin"), K("atan"), K("ceil"), K("cos"), K("deg"), K("exp"), K("tointeger"), K("floor"), K("fmod"), K("ult"), K("log"), K("max"), K("min"), K("modf"), K("rad"), K("random"), K("randomseed"), K("sin"), K("sqrt"), K("tan"), K("type"), K("atan2"), K("cosh"), K("sinh"), K("tanh"), K("pow"), K("frexp"), K("ldexp"), K("log10"), K("pi"), K("huge"), K("maxinteger"), K("mininteger")]),
    ("os", &[R(0, 11), K("clock"), K("date"), K("difftime"), K("execute"), K("exit"), K("getenv"), K("remove"), K("rename"), K("setlocale"), K("time"), K("tmpname")]),
    ("package", &[R(0, 7), K("loadlib"), K("searchpath"), K("preload"), K("cpath"), K("path"), K("searchers"), K("loaded"), K("config")]),
    ("package.searchers", &[R(4, 0)]),
    ("registry", &[R(2, 0), K("_LOADED"), K("_PRELOAD"), K("FILE*"), K("_IO_input"), K("_IO_output")]),
    ("registry._LOADED", &[K("_G"), K("package"), K("coroutine"), K("table"), K("io"), K("os"), K("string"), K("math"), K("utf8"), K("debug"), K("bit32")]),
    ("string", &[R(0, 17), K("byte"), K("char"), K("dump"), K("find"), K("format"), K("gmatch"), K("gsub"), K("len"), K("lower"), K("match"), K("rep"), K("reverse"), K("sub"), K("upper"), K("pack"), K("packsize"), K("unpack")]),
    ("string_mt", &[R(0, 1), K("__index")]),
    ("table", &[R(0, 7), K("concat"), K("insert"), K("pack"), K("unpack"), K("remove"), K("move"), K("sort")]),
    ("utf8", &[R(0, 6), K("offset"), K("codepoint"), K("char"), K("len"), K("codes"), K("charpattern")]),
];

pub(crate) static LAYOUT_54: &[(&str, &[LibOp])] = &[
    ("FILE*", &[R(0, 2), K("__name"), K("__index"), K("__gc"), K("__close"), K("__tostring")]),
    ("FILE*.__index", &[R(0, 7), K("read"), K("write"), K("lines"), K("flush"), K("seek"), K("close"), K("setvbuf")]),
    ("_G", &[K("assert"), K("collectgarbage"), K("dofile"), K("error"), K("getmetatable"), K("ipairs"), K("loadfile"), K("load"), K("next"), K("pairs"), K("pcall"), K("print"), K("warn"), K("rawequal"), K("rawlen"), K("rawget"), K("rawset"), K("select"), K("setmetatable"), K("tonumber"), K("tostring"), K("type"), K("xpcall"), K("_G"), K("_VERSION"), K("require"), K("package"), K("coroutine"), K("table"), K("io"), K("os"), K("string"), K("math"), K("utf8"), K("debug")]),
    ("coroutine", &[R(0, 8), K("create"), K("resume"), K("running"), K("status"), K("wrap"), K("yield"), K("isyieldable"), K("close")]),
    ("debug", &[R(0, 17), K("debug"), K("getuservalue"), K("gethook"), K("getinfo"), K("getlocal"), K("getregistry"), K("getmetatable"), K("getupvalue"), K("upvaluejoin"), K("upvalueid"), K("setuservalue"), K("sethook"), K("setlocal"), K("setmetatable"), K("setupvalue"), K("traceback"), K("setcstacklimit")]),
    ("io", &[R(0, 11), K("close"), K("flush"), K("input"), K("lines"), K("open"), K("output"), K("popen"), K("read"), K("tmpfile"), K("type"), K("write"), K("stdin"), K("stdout"), K("stderr")]),
    ("math", &[R(0, 35), K("abs"), K("acos"), K("asin"), K("atan"), K("ceil"), K("cos"), K("deg"), K("exp"), K("tointeger"), K("floor"), K("fmod"), K("ult"), K("log"), K("max"), K("min"), K("modf"), K("rad"), K("sin"), K("sqrt"), K("tan"), K("type"), K("atan2"), K("cosh"), K("sinh"), K("tanh"), K("pow"), K("frexp"), K("ldexp"), K("log10"), K("random"), K("randomseed"), K("pi"), K("huge"), K("maxinteger"), K("mininteger")]),
    ("os", &[R(0, 11), K("clock"), K("date"), K("difftime"), K("execute"), K("exit"), K("getenv"), K("remove"), K("rename"), K("setlocale"), K("time"), K("tmpname")]),
    ("package", &[R(0, 7), K("loadlib"), K("searchpath"), K("preload"), K("cpath"), K("path"), K("searchers"), K("loaded"), K("config")]),
    ("package.searchers", &[R(4, 0)]),
    ("registry", &[R(2, 0), K("_LOADED"), K("_CLIBS"), K("_PRELOAD"), K("FILE*"), K("_IO_input"), K("_IO_output")]),
    ("registry._LOADED", &[K("_G"), K("package"), K("coroutine"), K("table"), K("io"), K("os"), K("string"), K("math"), K("utf8"), K("debug")]),
    ("string", &[R(0, 17), K("byte"), K("char"), K("dump"), K("find"), K("format"), K("gmatch"), K("gsub"), K("len"), K("lower"), K("match"), K("rep"), K("reverse"), K("sub"), K("upper"), K("pack"), K("packsize"), K("unpack")]),
    ("string_mt", &[R(0, 9), K("__add"), K("__sub"), K("__mul"), K("__mod"), K("__pow"), K("__div"), K("__idiv"), K("__unm"), K("__index")]),
    ("table", &[R(0, 7), K("concat"), K("insert"), K("pack"), K("unpack"), K("remove"), K("move"), K("sort")]),
    ("utf8", &[R(0, 6), K("offset"), K("codepoint"), K("char"), K("len"), K("codes"), K("charpattern")]),
];

pub(crate) static LAYOUT_55: &[(&str, &[LibOp])] = &[
    ("FILE*", &[R(0, 2), K("__name"), K("__index"), K("__gc"), K("__close"), K("__tostring")]),
    ("FILE*.__index", &[R(0, 7), K("read"), K("write"), K("lines"), K("flush"), K("seek"), K("close"), K("setvbuf")]),
    ("_G", &[K("assert"), K("collectgarbage"), K("dofile"), K("error"), K("getmetatable"), K("ipairs"), K("loadfile"), K("load"), K("next"), K("pairs"), K("pcall"), K("print"), K("warn"), K("rawequal"), K("rawlen"), K("rawget"), K("rawset"), K("select"), K("setmetatable"), K("tonumber"), K("tostring"), K("type"), K("xpcall"), K("_G"), K("_VERSION"), K("require"), K("package"), K("coroutine"), K("debug"), K("io"), K("math"), K("os"), K("string"), K("table"), K("utf8")]),
    ("coroutine", &[R(0, 8), K("create"), K("resume"), K("running"), K("status"), K("wrap"), K("yield"), K("isyieldable"), K("close")]),
    ("debug", &[R(0, 16), K("debug"), K("getuservalue"), K("gethook"), K("getinfo"), K("getlocal"), K("getregistry"), K("getmetatable"), K("getupvalue"), K("upvaluejoin"), K("upvalueid"), K("setuservalue"), K("sethook"), K("setlocal"), K("setmetatable"), K("setupvalue"), K("traceback")]),
    ("io", &[R(0, 11), K("close"), K("flush"), K("input"), K("lines"), K("open"), K("output"), K("popen"), K("read"), K("tmpfile"), K("type"), K("write"), K("stdin"), K("stdout"), K("stderr")]),
    ("math", &[R(0, 29), K("abs"), K("acos"), K("asin"), K("atan"), K("ceil"), K("cos"), K("deg"), K("exp"), K("tointeger"), K("floor"), K("fmod"), K("frexp"), K("ult"), K("ldexp"), K("log"), K("max"), K("min"), K("modf"), K("rad"), K("sin"), K("sqrt"), K("tan"), K("type"), K("random"), K("randomseed"), K("pi"), K("huge"), K("maxinteger"), K("mininteger")]),
    ("os", &[R(0, 11), K("clock"), K("date"), K("difftime"), K("execute"), K("exit"), K("getenv"), K("remove"), K("rename"), K("setlocale"), K("time"), K("tmpname")]),
    ("package", &[R(0, 7), K("loadlib"), K("searchpath"), K("preload"), K("cpath"), K("path"), K("searchers"), K("loaded"), K("config")]),
    ("package.searchers", &[R(4, 0)]),
    ("registry", &[R(3, 0), K("_PRELOAD"), K("_LOADED"), K("_CLIBS"), K("FILE*"), K("_IO_input"), K("_IO_output")]),
    ("registry._LOADED", &[K("_G"), K("package"), K("coroutine"), K("debug"), K("io"), K("math"), K("os"), K("string"), K("table"), K("utf8")]),
    ("string", &[R(0, 17), K("byte"), K("char"), K("dump"), K("find"), K("format"), K("gmatch"), K("gsub"), K("len"), K("lower"), K("match"), K("rep"), K("reverse"), K("sub"), K("upper"), K("pack"), K("packsize"), K("unpack")]),
    ("string_mt", &[R(0, 9), K("__add"), K("__sub"), K("__mul"), K("__mod"), K("__pow"), K("__div"), K("__idiv"), K("__unm"), K("__index")]),
    ("table", &[R(0, 8), K("concat"), K("create"), K("insert"), K("pack"), K("unpack"), K("remove"), K("move"), K("sort")]),
    ("utf8", &[R(0, 6), K("offset"), K("codepoint"), K("char"), K("len"), K("codes"), K("charpattern")]),
];
