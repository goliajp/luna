-- v3.1 debug slice: an xpcall message handler runs where the error was
-- raised — before any __close handler, and seeing the function that raised
-- it (the C function 'error' included) — as PUC's luaG_errormsg does.
local log = {}
local function norm(tb)
  tb = tb:gsub("\n[^\n]*in main chunk.*$", "\n\t<main>")
  tb = tb:gsub("[%w_%.%-]+:%d+:", "SRC:")
  tb = tb:gsub("<[%w_%.%-]+:%d+>", "<SRC>")
  return tb
end
local function f() error("boom") end
print(norm(select(2, xpcall(f, debug.traceback))))
print(norm(select(2, xpcall(f, function(m) return debug.traceback(m, 1) end))))
print(select(2, xpcall(f, function(m)
  local i = debug.getinfo(2, "Sn")
  return i.what .. " " .. tostring(i.name)
end)))
print(select(2, xpcall(function() local x = nil; return x.y end, function(m)
  return debug.getinfo(2, "S").what
end)))
