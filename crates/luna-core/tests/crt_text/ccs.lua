-- 5.1 io.open passes ",ccs=..." to fopen. Run with a case number; each
-- case runs in its own process, since some end it. File names are under
-- the global DIR when it is set.
local D = DIR or ""
local function P(n) return D .. n end
io.stdout:setvbuf("no")
local function esc(s)
  if s == nil then return "nil" end
  if io.type(s) then return "file" end
  if type(s) ~= "string" then return tostring(s) end
  return (s:gsub("[%c\128-\255\\]", function(c) return string.format("\\%d", c:byte()) end))
end
local function r(...) local t = {} for i = 1, select("#", ...) do t[#t+1] = esc((select(i, ...))) end return table.concat(t, ",") end
local function raw(name) local f = assert(io.open(name, "rb")); local s = f:read("*a"); f:close(); return s end
local function put(name, s) local f = assert(io.open(name, "wb")); f:write(s); f:close() end
local CCS = { "UTF-8", "UTF-16LE", "UNICODE" }
local TEXT = { odd = "a\195\169\n\226\130\172x!", even = "a\195\169\n\226\130\172x", nl = "ab\ncd\n" }
local FILES = {
  ascii = "ab\r\ncd\n",
  utf8 = "a\195\169\r\n\226\130\172x",
  utf8bom = "\239\187\191a\195\169\r\n\226\130\172x",
  utf16bom = "\255\254a\0\233\0\r\0\n\0\172\32x\0",
  utf16 = "a\0\233\0\r\0\n\0\172\32x\0",
}
local cases = {}
for _, c in ipairs(CCS) do
  for _, m in ipairs({ "w", "a", "w+" }) do
    for _, t in ipairs({ "even", "odd", "nl" }) do
      cases[#cases+1] = function()
        os.remove(P("o.txt"))
        local f, e, n = io.open(P("o.txt"), m .. ", ccs=" .. c)
        if not f then return "open " .. r(f, e, n) end
        local w = r(f:write(TEXT[t])); local s = r(f:seek("cur")); local cl = r(f:close())
        return table.concat({ "write", c, m, t, w, s, cl, esc(raw(P("o.txt"))) }, " ")
      end
    end
  end
  for _, existing in ipairs({ "utf8bom", "utf16bom" }) do
    cases[#cases+1] = function()
      put(P("o.txt"), FILES[existing])
      local f = io.open(P("o.txt"), "a, ccs=" .. c); local w = r(f:write(TEXT.even)); f:close()
      return table.concat({ "append", c, existing, w, esc(raw(P("o.txt"))) }, " ")
    end
  end
  for _, k in ipairs({ "ascii", "utf8", "utf8bom", "utf16bom", "utf16" }) do
    for _, how in ipairs({ "*l", "*a", "2", "1", "0", "*n", "seek", "lines" }) do
      cases[#cases+1] = function()
        put(P("i.txt"), FILES[k])
        local f, e, n = io.open(P("i.txt"), "r, ccs=" .. c)
        if not f then return "open " .. r(f, e, n) end
        local t = {}
        if how == "seek" then
          t[#t+1] = r(f:seek("cur")); t[#t+1] = r(f:seek("set", 0)); t[#t+1] = r(f:seek("end"))
          t[#t+1] = r(f:seek("set", 2)); t[#t+1] = r(f:seek("set", 3))
        elseif how == "lines" then
          for l in f:lines() do t[#t+1] = esc(l) end
        else
          local fmt = tonumber(how) or how
          for i = 1, 4 do t[#t+1] = r(f:read(fmt)) .. "@" .. r(f:seek("cur")) end
        end
        f:close()
        return table.concat({ "read", c, k, how, table.concat(t, "|") }, " ")
      end
    end
  end
  cases[#cases+1] = function()
    put(P("u.txt"), FILES.utf16bom)
    local f = io.open(P("u.txt"), "r+, ccs=" .. c)
    local t = { r(f:read(2)), r(f:seek("cur", 0)), r(f:write("Zz")), r(f:seek("set", 0)), r(f:read("*a")) }
    f:close()
    return table.concat({ "update", c, table.concat(t, "|"), esc(raw(P("u.txt"))) }, " ")
  end
end
for _, m in ipairs({ "r, ccs=KOI8", "r,ccs=UTF-8", "rt, ccs=utf-8", "rb, ccs=UTF-8", "r, ccs=UTF-8, ccs=UTF-8", "r ccs=UTF-8" }) do
  cases[#cases+1] = function() put(P("i.txt"), "x\n"); return "mode " .. esc(m) .. " " .. r(io.open(P("i.txt"), m)) end
end
if arg[1] == "count" then print(#cases) else print(arg[1], cases[tonumber(arg[1])]()) end
