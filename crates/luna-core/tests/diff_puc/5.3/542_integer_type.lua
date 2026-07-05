-- v2.15 P2.5 (5.3): integer subtype introduced.
print(math.type(1))            -- integer
print(math.type(1.0))          -- float
print(math.type(1 + 0))        -- integer
print(math.type(1 + 0.0))      -- float
print(math.type(0 / 0))        -- float (nan)
print(math.type(1 / 1))        -- float (div always float)
print(math.type(1 // 1))       -- integer
print(math.type(1.0 // 1.0))   -- float (both float)
