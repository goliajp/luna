-- v2.11 CORPUS-II: return in different positions.
local function only()
  return 42
end
print(only())

-- return in middle
local function early(x)
  if x > 0 then return "pos" end
  return "neg"
end
print(early(5), early(-5))

-- multi-return in expression
local function two() return "a", "b" end
print(two())
print((two()))    -- parens adjust to 1: "a"

-- empty return
local function noret() return end
print(noret())    -- nothing (no output before newline)
