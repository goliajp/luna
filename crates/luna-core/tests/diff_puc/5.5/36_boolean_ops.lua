-- v2.10 CORPUS: and/or/not truthiness.
print(nil and 1)      -- nil
print(false and 1)    -- false
print(0 and 1)        -- 1 (0 is truthy in Lua)
print("" and 1)       -- 1 (empty string truthy)
print(1 or 2)         -- 1
print(nil or 2)       -- 2
print(false or "x")   -- x
print(not nil)
print(not false)
print(not 0)          -- false (0 is truthy)
print(not "")         -- false
print(not not 42)     -- true
