-- Variable info in runtime type errors, per dialect: constants are named
-- from 5.2 (but not as a binary operand before 5.4, which PUC takes
-- straight from the constant table), `t[1]` is "integer index" only from
-- 5.4, an indexed upvalue table (`_ENV`) is named, and 5.4+ name the
-- generic-for iterator.
local ld = loadstring or load
local function e(src) print(pcall(ld(src, "=c"))) end
-- 5.4+ report string arithmetic through the string metamethods instead
if _VERSION == "Lua 5.2" or _VERSION == "Lua 5.3" then e("return -'abc'") end
e("return ('x') * 2")
e("return 'abc' + 1")
e("return #5")
e("return ('abc')()")
e("local t = {} return t[1] + 1")
e("local t = {} return t[1].x")
e("local t = {} return t[1]()")
e("local t, k = {}, 'q' return t[k].x")
if _VERSION ~= "Lua 5.1" then
  print(pcall(load("x = 1", "=c", "t", nil)))
  print(pcall(load("return y.z", "=c", "t", nil)))
end
e("for k in nil do end")
e("for k in 3 do end")
e("local t = {} for k in t.x do end")
