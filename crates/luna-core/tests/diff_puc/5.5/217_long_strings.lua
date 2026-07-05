-- v2.13 CORPUS-IV: long-bracket strings — level nesting, leading
-- newline swallow, no escape processing.
local a = [[line1
line2]]
print(a)
local b = [[
swallowed]]
print(b)
local c = [==[ has ]] inside ]==]
print(c)
print([[no \n escapes \t here]])
print(#[[
]])
