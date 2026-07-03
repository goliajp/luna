-- v2.13 CORPUS-IV: and/or return operand VALUES, short-circuit.
print(1 and 2, nil and 2, false and 2)
print(1 or 2, nil or 2, false or "fb")
print(nil and nil, false or nil)
print(0 and "zero_is_true", "" and "empty_is_true")
local t = {}
print((t or error("not evaluated")) == t)
print((nil or {}) ~= nil)
