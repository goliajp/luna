-- what a hook sees of the C function it interrupts: its values up to the
-- hook, and (5.4 on) where a return transfers from, for debug.sethook
-- (which leaves the hook table pushed), pcall, xpcall, error and tostring
local out = {}
local function put(s) out[#out + 1] = s end
local function str(v)
  local t = type(v)
  if t == "function" or t == "table" or t == "thread" or t == "userdata" then return t end
  return tostring(v)
end
local function hook(ev)
  local i = debug.getinfo(2, "nSl" .. (_VERSION >= "Lua 5.4" and "r" or ""))
  if i.what ~= "C" then return end
  local s = ev .. " " .. tostring(i.name) .. " " .. tostring(i.namewhat)
  if i.ftransfer then
    s = s .. " r " .. i.ftransfer .. " " .. i.ntransfer
    for k = i.ftransfer, i.ftransfer + i.ntransfer - 1 do
      local _, v = debug.getlocal(2, k)
      s = s .. " " .. str(v)
    end
  end
  -- the last value is the hook table, which the debug library's
  -- trampoline (`hookf`) pushes above the C function before the hook
  local vals = {}
  for k = 1, 8 do
    local n, v = debug.getlocal(2, k)
    if not n then break end
    vals[#vals + 1] = str(v)
  end
  put(s .. " | " .. table.concat(vals, ","))
end
local function ok(a, b) return a, b, "r3" end
local function bad() error("e1", 0) end
local function h(m) return "h:" .. tostring(m) end
debug.sethook(hook, "cr")
debug.sethook(hook, "cr", 0)
pcall(ok, 1, 2)
pcall(bad)
xpcall(ok, h, 3, 4)
xpcall(bad, h)
pcall(pcall, ok, 5)
debug.sethook(nil, "x", 5)
debug.sethook(hook, "r")
debug.sethook()
for _, l in ipairs(out) do print(l) end
