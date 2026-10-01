-- Integers the VM keeps (#t, select('#')) stand for doubles: -0, rounding past 2^53, no wrap, and a key -0.
local z = #{}
local one = #{1}
local two = select('#', 1, 2)
local function show(...) local t = {} for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end print(table.concat(t, " ")) end
show("unm", -z, 1 / -z)
show("mul", z * -one, 1 / (z * -one), -one * z, 1 / (-one * z))
show("mulk", z * -1, 1 / (z * -1), -1 * z, 1 / (-1 * z))
show("mod", (-two) % two, 1 / ((-two) % two), two % (-two), 1 / (two % (-two)))
show("sub", z - z, 1 / (z - z), (-z) - z, 1 / ((-z) - z))
show("add", (-z) + (-z), 1 / ((-z) + (-z)), z + (-z), 1 / (z + (-z)))
show("float", -0.0, 1 / -0.0, 0.0 * -1, 1 / (0.0 * -1))
local t = {} t[-z] = "a" for k, v in pairs(t) do show("key", k, v, 1 / k) end
local u = {} u[z * -one] = "b" for k, v in pairs(u) do show("keymul", k, v, 1 / k) end
local w = {} w[-0.0] = "c" for k, v in pairs(w) do show("keyflt", k, v, 1 / k) end
local x = {} x[0] = "d" x[-z] = "e" for k, v in pairs(x) do show("keyboth", k, v) end
local y = {} y[-z] = "f" show("lookup", y[0], y[z], y[-0.0])
show("fmt", string.format("%g %d", -z, -z))
for i = -z, -z do show("for", i, 1 / i) end
show("lit", -0, 1 / -0, 0 * -1, 1 / (0 * -1), -(0), 1 / -(0))
local n0 = 0
show("litvar", -n0, 1 / -n0, n0 * -1, 1 / (n0 * -1), -1 * n0, 1 / (-1 * n0))
show("divmod", z / -one, 1 / (z / -one), z % -one, 1 / (z % -one))
show("pow", (-one) ^ one * z, 1 / ((-one) ^ one * z))
show("tostr", tostring(-z) .. "", -z .. "", string.format("%.1f", -z))
show("cmp", -z == z, -z < z, math.abs(-z), 1 / math.abs(-z))
show("fn", 1 / math.floor(-z), 1 / math.ceil(-z), 1 / math.max(-z, -z), 1 / math.min(-z))
show("str2n", 1 / ("-0" + 0), 1 / tonumber("-0"), 1 / (0 * -"1"))
local hot = 0 for i = 1, 200 do local q = -z hot = hot + 1 / q end show("hotunm", hot)
local hm = 0 for i = 1, 200 do local q = z * -one hm = hm + 1 / q end show("hotmul", hm)
local hk = {} for i = 1, 200 do hk[-z] = i end for k in pairs(hk) do show("hotkey", k, 1 / k) end
show("lib", 1 / math.ceil(-0.5), 1 / math.floor(-0.0), 1 / (math.modf(-0.5)), 1 / math.fmod(-0.0, 1), 1 / math.abs(-0.0))
show("modzero", tostring(one % z) == tostring(0/0), (-two) % z ~= (-two) % z)
local x2 = two for i = 1, 70 do x2 = x2 * two end print("mul", x2)
local y2 = one for i = 1, 64 do y2 = y2 + y2 end print("add", y2)
local s2 = -one for i = 1, 64 do s2 = s2 + s2 end print("addneg", s2)
local u2 = one for i = 1, 63 do u2 = u2 * two end print("unm", -u2, -(-u2))
local p2 = one for i = 1, 53 do p2 = p2 * two end print("round", p2 + one == p2, string.format("%.0f", p2 + one))
