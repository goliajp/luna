-- `X and nil or 0` is the constant 0 to PUC's code generator whatever X
-- is: X still runs, but the `or` is left with the constant. From 5.4,
-- `x - 0` with a constant 0 runs as `x + 0`, so a -0.0 on the left comes
-- out as 0.0
local x = -0.0
local a, calls = true, 0
local function f() calls = calls + 1 return true end
print(x - ((nil) ~= 0 and (nil) or 0))
print(x - (a and nil or 0))
print(x - (f() and false or 0))
print(x - (a and (a and nil) or 0))
print(x - ((-9866399) % ((nil) ~= 0 and (nil) or 1)))
print(x - (a and 0 or 0), x - (nil or 0), x - (not a or 0))
print(calls)
