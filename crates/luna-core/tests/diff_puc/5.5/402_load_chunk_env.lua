-- v2.14 CV.3: load with custom chunkname affects error position
-- prefix (structure only) + mode restrictions.
local f = load("return 40 + 2", "=mychunk")
print(f())
local bad, err = load("syntax ~!", "=mychunk")
print(bad == nil, err:match("mychunk") ~= nil)
local blocked = load("return 1", "c", "b")
print(blocked == nil)
local make = function()
  local parts = { "return ", "7", " * 6" }
  local i = 0
  return function() i = i + 1 return parts[i] end
end
local fn = load(make())
print(fn())
