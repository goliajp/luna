-- run as a script file
-- library functions that raise an error with their buffer's box on the
-- stack: the box is a to-be-closed slot, which the protected call that
-- catches the error closes, its __close called above the box with the
-- room a C function's call takes; the box is garbage from then on
collectgarbage("stop")
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
local big = string.rep("x", 2000)
local function try(name, f, ...)
  collectgarbage()
  local base = count()
  local ok, err = pcall(f, ...)
  local made = count() - base
  collectgarbage()
  print(name, ok, err, made, count() - base)
end
try("concat", table.concat, {big, {}})
try("format", string.format, "%s%d", big, {})
try("gsub", string.gsub, big, "x$", function() error("stop") end)
-- the same error from frames of each size up to 60 registers, so that
-- the room the __close call takes is past the stack's end for some
local function frame(n)
  local names = {}
  for i = 1, n do names[i] = "v" .. i end
  local decl = n > 0 and ("local " .. table.concat(names, ", ") .. " ") or ""
  return load(decl .. "return pcall(...)")
end
local function sweep(name, ...)
  local grown = {}
  for n = 0, 60 do
    local f = frame(n)
    collectgarbage()
    local base = count()
    f(...)
    grown[#grown + 1] = count() - base
  end
  print(name, table.concat(grown, " "))
end
sweep("concat", table.concat, {big, {}})
sweep("format", string.format, "%s%d", big, {})
sweep("gsub", string.gsub, big, "x$", function() error("stop") end)
