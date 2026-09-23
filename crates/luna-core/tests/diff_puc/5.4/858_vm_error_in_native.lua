-- An error the VM raises while a native function is running carries no
-- position (luaG_runerror adds one only for a Lua activation) and names
-- no variable; the same error raised by a Lua function has both.
local ld = loadstring or load
local function e(src) print(pcall(ld(src, "=c"))) end
e("rawset({}, nil, 1)")
e("local k = 0/0 rawset({}, k, 1)")
e("table.sort({3, 'x', 1})")
e("local a, b = {}, {} table.sort({a, b})")
e("local t = {} t[nil] = 1")
e("return 1 < 'x'")
