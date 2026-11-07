//! The fixed frame of a `fuzz_jit_diff` program: the globals, tables and
//! metatables every program starts with, and the dump that ends it.

pub(crate) const FRAME: &str = r#"__out = {}
print = function(...)
  local p = {}
  for i = 1, select('#', ...) do p[i] = tostring((select(i, ...))) end
  __out[#__out + 1] = table.concat(p, "\t")
end
local n1, n2, n3 = NUM1, NUM2, NUM3
local x, s = nil, "s"
local cnt, psum, hits, nhits = 0, 0, 0, 0
local c = {0, 0, n = 0}
local t = {1, 2, 3, x = 1, y = "y"}
local arr = {1, 2.5, "3", 4, 5, 6, 7, 8}
local seq = {}
local Mstore = {}
local MT = {}
MT.__index = function(_, k)
  hits = hits + 1
  local v = Mstore[k]
  if v == nil then return type(k) == "number" and k * 2 or 1 end
  return v
end
MT.__newindex = function(_, k, v) nhits = nhits + 1 Mstore[k] = v end
MT.__len = function() hits = hits + 1 return 7 end
MT.__concat = function(p, q) hits = hits + 1 return type(p) .. type(q) end
MT.__eq = function() hits = hits + 1 return hits % 2 == 0 end
MT.__lt = function() hits = hits + 1 return hits % 3 == 0 end
MT.__le = function() hits = hits + 1 return hits % 3 ~= 0 end
MT.__call = function(_, y) hits = hits + 1 return (tonumber(y) or 0) + 1 end
MT.__add = function() hits = hits + 1 return 3 end
MT.__unm = function() hits = hits + 1 return -1 end
local MTidx = MT.__index
local M, M2 = setmetatable({}, MT), setmetatable({}, MT)
local P = setmetatable({}, {__index = t, __newindex = t})
local TMT = {__index = function(_, k) hits = hits + 1 return 5 end}
local TMT2 = {__index = Mstore, __newindex = Mstore}
local objs = {t, M, P}
local BT, BK = {a = false, b = true, c = false}, {"a", "b", "c"}
local o = t
local obj = {v = 0}
local function va(...)
  local acc = select('#', ...)
  for q = 1, acc do
    local v = select(q, ...)
    if type(v) == "number" then acc = acc + v end
  end
  return acc, ...
end
"#;

// addresses are masked before the sort: with a function or table key the
// order would otherwise follow allocation addresses, which differ between
// the jit and interpreter runs
pub(crate) const DUMP: &str = r#"local function dump(tb)
  local ks = {}
  for k, v in pairs(tb) do
    ks[#ks + 1] = (string.gsub(type(k):sub(1, 1) .. tostring(k) .. "=" .. tostring(v), "0[xX]%x+", "ADDR"):gsub("%x%x+[xX]0", "ADDR"))
  end
  table.sort(ks)
  return table.concat(ks, " ")
end
print("end", n1, n2, n3, x, s, cnt, psum, hits, nhits, #seq, obj.v)
print(dump(t)) print(dump(Mstore)) print(dump(arr)) print(dump(c))
"#;

/// Addresses (`table: 0x…`) differ between two Vms.
pub(crate) fn mask_addresses(s: &str) -> String {
    // the program can also print an address upper-cased (`0X7B…`) or
    // reversed (`…b7x0`)
    let b = s.as_bytes();
    let hex = |k: usize| b.get(k).is_some_and(u8::is_ascii_hexdigit);
    let x = |k: usize| matches!(b.get(k), Some(b'x' | b'X'));
    let mut out = String::with_capacity(s.len());
    let mut start = 0;
    let mut k = 0;
    while k < b.len() {
        if b[k] == b'0' && x(k + 1) && hex(k + 2) {
            let mut e = k + 2;
            while hex(e) {
                e += 1;
            }
            out.push_str(&s[start..k]);
            out.push_str("ADDR");
            start = e;
            k = e;
        } else if hex(k) && !hex(k.wrapping_sub(1)) {
            let mut e = k;
            while hex(e) {
                e += 1;
            }
            if x(e) && b.get(e + 1) == Some(&b'0') && e - k >= 2 {
                out.push_str(&s[start..k]);
                out.push_str("ADDR");
                start = e + 2;
                k = e + 2;
            } else {
                k = e;
            }
        } else {
            k += 1;
        }
    }
    out.push_str(&s[start..]);
    out
}
