-- v2.13 CORPUS-IV: assert returns ALL its arguments on success;
-- custom message objects pass through on failure.
print(assert(1, 2, 3))
print(assert("v"))
local ok, err = pcall(assert, false, "custom_msg")
print(ok, err)
local sentinel = {}
local ok2, err2 = pcall(assert, nil, sentinel)
print(ok2, err2 == sentinel)
local ok3, err3 = pcall(assert, false)
print(ok3, err3)
