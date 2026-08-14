-- v2.18 P3: how the table library treats a non-table argument, and whether
-- it reads elements raw or through metamethods. PUC changed this twice:
--
--   5.1  hard luaL_checktype + lua_rawgeti/rawseti; __len ignored for tables
--   5.2  hard luaL_checktype + lua_rawgeti/rawseti; __len honoured
--   5.3+ checktab (duck-typed on __index/__newindex/__len) + lua_geti/seti
--   5.5  additionally exempts strings from the __len requirement
--
-- luna previously used a hard check together with metamethod access, so it
-- matched no dialect exactly.
--
-- Error text is compared with the "chunkname:line: " prefix stripped: the
-- harness wraps each side differently, and long paths are truncated at
-- different offsets. `table.unpack` is deliberately not exercised here --
-- PUC names the *caller's local* in its argument errors, which is a
-- separate divergence and would mask this one.
local function p(name, f)
  local ok, r = pcall(f)
  print(name, ok, (tostring(r):gsub("^.-:%d+: ", "")))
end
-- a proxy whose contents live behind __index
local raw = {3, 1, 2}
local prox = setmetatable({}, {
  __index = raw,
  __newindex = function(_, k, v) raw[k] = v end,
  __len = function() return #raw end,
})
p("concat_proxy", function() return table.concat(prox, ",") end)
p("len_proxy", function() return #prox end)
p("sort_proxy", function() table.sort(prox) return table.concat(raw, ",") end)
-- non-table subjects
p("concat_string", function() return table.concat("abc") end)
p("insert_string", function() return table.insert("abc", 1) end)
p("sort_string", function() return table.sort("abc") end)
p("concat_number", function() return table.concat(42) end)
p("insert_number", function() return table.insert(42, 1) end)
-- a plain table is always fine
p("concat_plain", function() return table.concat({1, 2, 3}, "-") end)
p("len_plain", function() return #({1, 2, 3}) end)
