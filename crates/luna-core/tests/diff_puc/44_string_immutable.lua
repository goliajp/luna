-- v2.10 CORPUS: string immutability + interning.
local s1 = "hello"
local s2 = "hel" .. "lo"
print(s1 == s2)   -- true (equal-value)
print(rawequal(s1, s2))  -- also true

-- concat creates new
local s3 = s1
s3 = s3 .. "!"
print(s1, s3)
print(s1 == s3)   -- false
