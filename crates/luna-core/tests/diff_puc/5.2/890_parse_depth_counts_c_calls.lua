-- The parser counts its nesting on top of the C calls already running
-- (PUC `enterlevel` uses the thread's nCcalls): a chunk loaded deep in
-- pcalls or in C-to-Lua callbacks has less room, and a module that
-- requires itself fails to *load* before the call that would run it
-- overflows, so `require` reports an error loading the module. A long
-- multiple assignment is bounded by the same budget. Depths are printed
-- relative to the room at the start, which depends on how the host runs
-- the script.
local load = loadstring or load
local pack = table.pack or function(...) return {n = select("#", ...), ...} end
local unpack = table.unpack or unpack

local function maxparen()
  for d = 100, 260 do
    local f, e = load("return " .. ("("):rep(d) .. "1" .. (")"):rep(d))
    if not f then return d - 1, (e:gsub("^.-:%d+: ", "")) end
  end
end
local function maxassign()
  for n = 100, 260 do
    local f, e = load(("a,"):rep(n - 1) .. "a = 1", "=assign")
    -- 5.1 quotes the room left, which depends on the host
    if not f then return n - 1, (e:gsub("%d+ variables", "N variables")) end
  end
end
local function viapcall(n, f)
  if n == 0 then return f() end
  local r = pack(pcall(viapcall, n - 1, f))
  return unpack(r, 2, r.n)
end
local function viagsub(n, f)
  if n == 0 then return f() end
  local r
  string.gsub("x", "x", function() r = pack(viagsub(n - 1, f)) end)
  return unpack(r, 1, r.n)
end

local function main()
  local room, msg = maxparen()
  print("parens", msg)
  for _, n in ipairs({1, 10, 40}) do
    local d, e = viapcall(n, maxparen)
    print("pcall", n, room - d, e)
    d, e = viagsub(n, maxparen)
    print("gsub", n, room - d, e)
  end
  local vars, e = maxassign()
  print("assign", room - vars, e)
  vars, e = viapcall(10, maxassign)
  print("assign in pcalls", room - vars, e)

  local file = os.tmpname()
  local h = assert(io.open(file, "w"))
  h:write("return require('m9')")
  h:close()
  package.path = file
  local ok, err = pcall(require, "m9")
  os.remove(file)
  print(ok, (err:gsub(file:gsub("%p", "%%%0"), "<file>")))
end
print(pcall(main))
