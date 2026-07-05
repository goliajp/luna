-- v2.15 P2.5 (5.3): math.tointeger.
print(math.tointeger(1))
print(math.tointeger(1.0))
print(math.tointeger(1.5))       -- nil (non-integer float)
print(math.tointeger("42"))       -- 42
print(math.tointeger("hello"))    -- nil
print(math.tointeger(math.huge))  -- nil
