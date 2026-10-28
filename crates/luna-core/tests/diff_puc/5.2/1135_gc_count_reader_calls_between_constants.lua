-- a reader handing over a table of numbers a few bytes at a time: between
-- its calls the parser codes items and adds constants at tokens that make
-- no string, so what each call allocates comes before what the parser
-- allocates for the tokens read after it
local parts = {"return {"}
for i = 1, 60 do parts[#parts + 1] = i * 7 + 0.5 .. ", " end
parts[#parts + 1] = "}"
local src = table.concat(parts)
local out = {}
for _, n in ipairs({3, 7, 11}) do
  local pos = 0
  local function reader()
    if pos >= #src then return nil end
    local s = src:sub(pos + 1, pos + n)
    pos = pos + n
    return s
  end
  collectgarbage()
  collectgarbage("stop")
  local before = collectgarbage("count")
  local f = assert(load(reader))
  local after = collectgarbage("count")
  collectgarbage("restart")
  out[#out + 1] = string.format("%d %.0f", #f(), (after - before) * 1024)
  f = nil
end
print(table.concat(out, " | "))
