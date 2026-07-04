-- v2.10 CORPUS: load + compile.
local f, err = load("return 40 + 2")
print(err, f())
local g = load("return function(x) return x * 3 end")
print(g()(4))  -- 12

-- load with parse error
local bad = load("this is not lua")
print(bad ~= nil)  -- may be nil OR function-that-errors
