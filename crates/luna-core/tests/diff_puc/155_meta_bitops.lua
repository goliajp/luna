-- v2.13 CORPUS-IV: bitwise metamethods (5.3+).
local names = {}
local mt = {
  __band = function() return "band" end,
  __bor = function() return "bor" end,
  __bxor = function() return "bxor" end,
  __bnot = function() return "bnot" end,
  __shl = function() return "shl" end,
  __shr = function() return "shr" end,
}
local o = setmetatable({}, mt)
print(o & 1, o | 1, o ~ 1)
print(~o)
print(o << 1, o >> 1)
print(1 & o, 1 | o)
