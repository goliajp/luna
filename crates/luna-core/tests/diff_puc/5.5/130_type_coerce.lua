-- v2.12 CORPUS-III: implicit string↔number coercion in arithmetic.
print("3" + 4)    -- 7 (str→num)
print("3" * "4")   -- 12
print("3.14" + 0)  -- 3.14
-- concat coerces number→string
print(1 .. 2)      -- "12"
print(1 .. "x")    -- "1x"
-- but comparison DOES NOT coerce
print("3" == 3)    -- false
