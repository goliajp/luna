-- v2.15 P2.5 (5.3): tostring distinguishes integer vs float.
print(tostring(1))      -- "1"
print(tostring(1.0))    -- "1.0"
print(tostring(2 + 2))  -- "4"
print(tostring(2.0 + 2))-- "4.0"
