-- What `load` holds on the stack below its reader while it undumps a
-- binary chunk, slot by slot. A string of more than 30 bytes is shown by
-- its length only: the long string 5.3 and 5.4 hold while they read its
-- bytes has whatever its memory had in PUC. 5.5's anchor table keys the
-- closure by itself, hashed by its address: its entries are sorted.
local out = {}
local function show(v)
  local tv = type(v)
  if tv == "string" then return #v > 30 and ("string#" .. #v) or string.format("%q", v) end
  if tv == "number" or tv == "boolean" or tv == "nil" then return tostring(v) end
  if tv == "function" then
    local i = debug.getinfo(v, "Su")
    if i.what ~= "main" then return "function" end
    return "function<" .. i.what .. " " .. i.source .. " " .. i.linedefined .. " " .. i.lastlinedefined .. " " .. i.nups .. ">"
  end
  if tv == "table" then
    local e = {}
    for k, x in pairs(v) do
      e[#e + 1] = (type(k) == "function" and rawequal(k, x)) and "self" or (show(k) .. "=" .. show(x))
    end
    table.sort(e)
    return "table[" .. table.concat(e, " ") .. "]"
  end
  return tv
end
local src = "local a = function(p, ...) local b = function() return \"" .. string.rep("L", 50) .. "\" end return b end return a, \"x\", 1.5, 2^53"
local bin = string.dump((loadstring or load)(src, "=bin"))
for _, step in ipairs({1, 5, 16, 1000}) do
  local k = 0
  local f = load(function()
    k = k + 1
    local line = {}
    for i = 1, 30 do
      local n, v = debug.getlocal(2, i)
      if not n then break end
      if type(v) == "string" and #v <= 30 and v:find("[^ -~]") then
        -- a piece of the chunk, in hex
        line[#line + 1] = "x" .. v:gsub(".", function(c) return string.format("%02x", c:byte()) end)
      else
        line[#line + 1] = show(v)
      end
    end
    out[#out + 1] = step .. " " .. k .. " " .. table.concat(line, " ")
    local p = bin:sub((k - 1) * step + 1, k * step)
    if p == "" then return nil end
    return p
  end, "=b")
  out[#out + 1] = tostring(f ~= nil)
end
return table.concat(out, "\n")
