-- v2.12 CORPUS-III: string.pack/unpack string formats —
-- zero-terminated ("z") + length-prefixed ("s1").
local z = string.pack("z", "hello")
print(#z, string.byte(z, #z))
local s, pos = string.unpack("z", z)
print(s, pos)
local p = string.pack("s1", "abc")
print(#p, string.byte(p, 1))
local q, qpos = string.unpack("s1", p)
print(q, qpos)
print(string.pack("zz", "a", "bc"):byte(1, -1))
