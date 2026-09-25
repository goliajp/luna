-- v3.1 debug slice: C functions are stack levels of their own — pcall,
-- xpcall and a native that calls back into Lua — and PUC names them from
-- the calling instruction; metamethods called by an instruction add none.
local function desc(i)
  if not i then return "nil" end
  return i.what .. ":" .. tostring(i.name) .. ":" .. i.namewhat
end
print(desc(select(2, pcall(debug.getinfo, 1, "Sn"))))
print(desc(select(2, pcall(debug.getinfo, 2, "Sn"))))
print(desc(select(2, xpcall(debug.getinfo, print, 1, "Sn"))))
print(desc(select(2, pcall(function() return debug.getinfo(2, "Sn") end))))
local s
table.sort({2, 1}, function(a, b) s = s or debug.getinfo(2, "Sn"); return a < b end)
print(desc(s))
string.gsub("x", "x", function() s = debug.getinfo(2, "Sn") end)
print(desc(s))
local o = setmetatable({}, {__index = function() return debug.getinfo(2, "Sn") end,
                            __add = function() return debug.getinfo(1, "Sn") end})
print(desc(o.x))
print(desc(o + 1))
print(select("#", pcall(debug.getlocal, 1, 1)), (pcall(debug.getlocal, 1, 1)))
