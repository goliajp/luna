-- What `load` holds on the stack below its reader at each call, slot by
-- slot: its arguments, the piece it keeps, and the parser's objects, with
-- the entries of the parser's tables in `next` order. Returns the listing.
local out = {}
local function show(v, deep)
  local tv = type(v)
  if tv == "string" then return string.format("%q", v) end
  if tv == "number" or tv == "boolean" or tv == "nil" then return tostring(v) end
  if tv == "table" and deep then
    -- a table keyed in a parser table (5.5's per-function constants):
    -- its entries sorted, as some are keyed by an address
    local e = {}
    for k, x in pairs(v) do e[#e + 1] = show(k) .. "=" .. show(x) end
    table.sort(e)
    return "table[" .. table.concat(e, " ") .. "]"
  end
  if tv == "function" and deep then
    local info = debug.getinfo(v, "S")
    return "function<" .. info.what .. " " .. info.source .. ">"
  end
  return tv
end
local function dump(tag)
  local line = {}
  for i = 1, 40 do
    local name, v = debug.getlocal(3, i)
    if not name then break end
    local s = show(v)
    if type(v) == "table" then
      local e = {}
      for k, x in pairs(v) do
        -- an object keyed by itself (5.5 anchors them so)
        local same = type(k) == "table" or type(k) == "function"
        e[#e + 1] = show(k, true) .. "=" .. (same and rawequal(k, x) and "self" or show(x))
      end
      s = s .. "{" .. table.concat(e, " ") .. "}"
    elseif type(v) == "function" then
      local info = debug.getinfo(v, "Su")
      if info.what == "main" then
        s = s .. "<" .. info.source .. " " .. info.linedefined .. " "
          .. info.lastlinedefined .. " " .. tostring(info.nups) .. ">"
      end
    end
    line[#line + 1] = s
  end
  out[#out + 1] = tag .. " " .. table.concat(line, " ")
end
local parts = {
  "local x = 10 local s = \"str\" ",
  "local t = {1, 2.5, a = \"k\", [3] = true, -4, 1e300} ",
  "local f = function(a, b) local y = a + 1 return y * 2 + b .. \"z\" end ",
  "x = x + 1 - 2 * 3 s = [[long ",
  "string that goes on]] t.a = t.a .. s ",
  "local g = function() local h = function() return 7, \"u\", 8.5 end return h end ",
  "for i = 1, 3 do x = x + i end ",
  "return f(x, 1), t, g",
}
if _VERSION >= "Lua 5.4" then
  table.insert(parts, #parts, "local c <const> = 42 local d = c + 100 - 1 ")
end
local src = table.concat(parts)
for _, step in ipairs({1, 7, 0}) do
  local k = 0
  local f, err = load(function()
    k = k + 1
    dump("s" .. step .. " r" .. k)
    if step == 0 then return parts[k] end
    if step == 1 and k == 13 then return src:sub(13) end
    if step == 1 and k > 13 then return nil end
    local p = src:sub((k - 1) * step + 1, k * step)
    if p == "" then return nil end
    return p
  end, "=chunk")
  out[#out + 1] = f and "ok" or ("fail " .. tostring(err))
end
return table.concat(out, "\n")
