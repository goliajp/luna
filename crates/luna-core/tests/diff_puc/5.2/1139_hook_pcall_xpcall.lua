-- pcall and xpcall are C functions: the call and return hooks run for
-- them, with them as level 2 of the hook; what the hook sees there
local out = {}
local function put(s) out[#out + 1] = s end
local function str(v)
  local t = type(v)
  if t == "function" or t == "table" or t == "thread" or t == "userdata" then return t end
  return tostring(v)
end
local function hook(ev)
  local i = debug.getinfo(2, "nSl" .. (_VERSION >= "Lua 5.4" and "r" or ""))
  if i.what ~= "C" or i.name == "sethook" then return end
  local s = ev .. " " .. tostring(i.name) .. " " .. tostring(i.namewhat)
  if i.ftransfer then
    s = s .. " r " .. i.ftransfer .. " " .. i.ntransfer
    for k = i.ftransfer, i.ftransfer + i.ntransfer - 1 do
      local _, v = debug.getlocal(2, k)
      s = s .. " " .. str(v)
    end
  end
  -- the last value is the table PUC's hookf pushes above the C function
  local vals = {}
  for k = 1, 8 do
    local n, v = debug.getlocal(2, k)
    if not n then break end
    vals[#vals + 1] = str(v)
  end
  vals[#vals] = nil
  put(s .. " | " .. table.concat(vals, ","))
end
local function ok(a, b) return a, b, "r3" end
local function bad() error("e1", 0) end
local function h(m) return "h:" .. tostring(m) end
debug.sethook(hook, "cr")
pcall(ok, 1, 2)
pcall(bad)
xpcall(ok, h, 3, 4)
xpcall(bad, h)
pcall(pcall, ok, 5)
debug.sethook()
for _, l in ipairs(out) do print(l) end
-- nested protected calls, counted by the call hook
do
  local n = 0
  local function f(k) if k > 0 then pcall(f, k - 1) end end
  debug.sethook(function () n = n + 1 end, "c")
  f(5)
  debug.sethook()
  local a = n
  n = 0
  debug.sethook(function () n = n + 1 end, "c")
  f(150)
  debug.sethook()
  local b = n
  n = 0
  debug.sethook(function () n = n + 1 end, "c")
  f(196)
  debug.sethook()
  print(a, b, n)
end
