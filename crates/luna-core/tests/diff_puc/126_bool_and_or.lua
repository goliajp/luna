-- v2.12 CORPUS-III: and/or return actual values, not boolean.
print(nil or "default")
print("value" or "default")
print(false or nil)     -- nil
print(nil or false)     -- false
print(nil and "x")      -- nil
print(false and "x")    -- false
print(0 and "x")        -- x (0 truthy)
print(1 or nil)         -- 1
