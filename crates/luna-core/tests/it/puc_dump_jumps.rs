//! Conditions, `and` / `or` / `not` and comparisons as values, `if` chains,
//! loops, `break` and `goto` list under `luac -l -l` as PUC's own
//! compilation of the same source: the same tests, the same jump lists
//! (5.1–5.3 join jumps to a jump into its list, 5.4+ send them on to its
//! target), the same closing of captured locals on the way out.

use super::puc_dump_listing::{DIALECTS, compare, frames};
use luna_core::version::LuaVersion;

const VALUES: &[&str] = &[
    "local a, b, c, d = ...\nlocal x = a and b or c and d\nlocal y = (a and b) or (c and not d)\n\
     local z = not not a\nlocal w = not (a == b)\nlocal v = a == b and c or d\n\
     local u = a < b or b <= c and c > d\nlocal t = {a and b, a or b, not a, a ~= b}\n\
     t[a and b] = c or d\nprint(a or b, a and b, a == b)\nreturn x, y, z, w, v, u, t\n",
    "local a, b = ...\nlocal x = 1 and 2\nlocal y = nil or false\nlocal z = true and nil\n\
     local w = false or a\nlocal v = a and 1 or 2\nlocal u = (a or 1) + 2\nif true then b = 1 end\n\
     if nil then b = 2 end\nif 1 == 1 then b = 3 end\nwhile false do b = 4 end\n\
     repeat b = 5 until true\nreturn x, y, z, w, v, u\n",
    "local a, b = ...\nlocal x = a == b\nlocal y = not a\nlocal z = a < b or b\n\
     local w = not (a and b)\nreturn x, y, z, w\n",
    "local a, b, c = ...\nlocal x = a and\n  b or\n  c\nif a and\n   b then\n  c = 1\nend\n\
     local y = a ==\n  b\nreturn x, y\n",
    "local function aux() return _ENV[1 < 2] end\nlocal a, b = ...\nlocal t = {}\n\
     t[a == b] = a ~= b\nreturn aux, t\n",
    "local a, b, c = ...\nlocal s\ns = a .. b\nc = s .. 'x' .. a\nlocal t = {}\nt.k = a .. b\n\
     return s, c, (a or b) .. c\n",
];

const FLOW: &[&str] = &[
    "local a, b, c = ...\nif a and b or c then return 1 end\nwhile a and not b do a = c end\n\
     if a == 1 or b == 2 then a = 3 elseif a < b and b > 4 then b = 5 else a = b end\n\
     return a ~= nil and b ~= nil\n",
    "local a, b, c = ...\nif not a then b = 1 end\nif a and (b or c) then c = 2 end\n\
     if (a or b) and c then a = 1 else b = 2 end\nreturn a ~= b and 1 or 2\n",
    "local a, b, c = ...\nif a then\n  b = 1\nelseif b then\n  c = 2\nelseif c then\n  a = 3\n\
     else\n  a = 4\nend\nwhile a do\n  if b then\n    c = 1\n  else\n    c = 2\n  end\nend\n\
     for i = 1, 3 do\n  if a then b = i end\nend\nreturn a\n",
    "local t = {...}\nfor i = 1, 10 do\n  if t[i] then break end\n  t[i] = i\nend\n\
     while true do\n  local x = t[1]\n  if x == nil then break end\n  t[1] = x - 1\nend\n",
    "local t = {...}\nfor i = 1, 10 do\n  local f = function() return i end\n\
     if t[i] then break end\n  t[i] = f\nend\nrepeat\n  local x = t[1]\n\
     local g = function() return x end\n  if x then break end\nuntil g()\n",
    "local t = {...}\nfor k, v in pairs(t) do\n  local g = function() return v end\n\
     if v then break end\n  t[k] = g\nend\nwhile true do\n  local x = t[1]\n\
     local f = function() return x end\n  if x then break end\n  t[1] = f\nend\n",
];

/// `goto`, from 5.2 on.
const GOTOS: &[&str] = &[
    "local a, b = ...\nfor i = 1, 3 do\n  if a then goto cont end\n  b = i\n  ::cont::\nend\n\
     while a do\n  if b then break elseif a == 1 then a = 2 end\nend\nreturn b\n",
    "local t = {...}\nfor i = 1, #t do\n  local x = t[i]\n  local f = function() return x end\n\
     if x == 1 then break end\n  if x == 2 then\n    t[i] = f\n    break\n  end\n  do\n\
         local y = x\n    local g = function() return y end\n    if y then break end\n  end\nend\n\
     while #t > 0 do\n  local z = t[1]\n  local h = function() return z end\n\
     if z then goto out end\n  t[1] = h\nend\n::out::\nrepeat\n  local q = t[2]\n\
     local k = function() return q end\nuntil q or k()\nreturn t\n",
    "local a, b = ...\ndo\n  local x = a\n  local f = function() return x end\n\
     if b then goto skip end\n  a = f\nend\n::skip::\nfor i = 1, 2 do\n\
     if a then goto continue end\n  b = i\n  ::continue::\nend\n::top::\nif a then\n  a = nil\n\
     goto top\nend\nreturn a, b\n",
];

#[test]
fn jumps_compile_as_puc_compiles_them() {
    let mut failed = Vec::new();
    for &(version, var) in DIALECTS {
        let Some(luac) = std::env::var(var).ok().filter(|s| !s.is_empty()) else {
            assert!(
                std::env::var_os("LUNA_DIFF_PUC_REQUIRE_ALL").is_none(),
                "{var} is not set"
            );
            continue;
        };
        let gotos: &[&str] = if version == LuaVersion::Lua51 {
            &[]
        } else {
            GOTOS
        };
        let cases = VALUES.iter().chain(FLOW).chain(gotos);
        failed.extend(cases.filter_map(|src| {
            compare(version, &luac, src).or_else(|| frames(version, &luac, src))
        }));
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
