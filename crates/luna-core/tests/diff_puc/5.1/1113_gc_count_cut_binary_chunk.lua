-- binary chunks cut short at many places: what PUC's undumper made of the
-- bytes it read (prototypes, their arrays, strings, a long string it had
-- begun) stays as garbage in the count, and the message is made as PUC
-- makes it
local function sample(a, b)
  local t = {x = "some string", y = 2.5, z = 3}
  local function inner(c) return c .. "inner" .. a .. "a string longer than forty bytes" end
  return inner(b), t
end
local whole = string.dump(sample)
local load = loadstring or load
local out = {}
for n = 1, #whole - 1, 3 do
  for _, name in ipairs({"=cut", whole}) do
    collectgarbage()
    collectgarbage("stop")
    local before = collectgarbage("count") * 1024
    local _, f, msg = pcall(load, whole:sub(1, n), name == whole and whole:sub(1, n) or name)
    local after = collectgarbage("count") * 1024
    collectgarbage("restart")
    out[#out + 1] = n .. ":" .. string.format("%.0f", after - before)
  end
end
print(#whole, table.concat(out, " "))
