-- the same labels in several functions of one chunk: no prototype keeps a
-- label's name, so once a function ends (and with it, up to 5.2, its table
-- of names) a collection may free the name, and the next function's scanner
-- makes it again. The count read after each load follows
local parts = {}
for i = 1, 12 do
  parts[#parts + 1] = string.format([[
local function f%d(n)
  local s = 0
  for i = 1, n do
    if i %% 2 == 0 then goto continue end
    s = s + i
    ::continue::
  end
  do goto done end
  s = -1
  ::done::
  return s
end
out[#out + 1] = f%d(%d)]], i, i, i)
end
local src = "local out = {}\n" .. table.concat(parts, "\n") .. "\nreturn table.concat(out, ',')"
-- an explicit chunk name: the default one is the source text, which PUC
-- caches by the address of its bytes, so whether a reload copies it again
-- would depend on the heap layout
collectgarbage()
collectgarbage("stop")
for round = 1, 3 do
  local before = collectgarbage("count")
  local f = assert(load(src, "=labels"))
  print(round, f(), collectgarbage("count") - before)
end
collectgarbage("restart")
for round = 1, 20 do
  local junk = {}
  for j = 1, 40 do junk[j] = "j" .. round .. "_" .. j end
  local f = assert(load(src, "=labels"))
  if round % 5 == 0 then print(round, f()) end
end
