-- Compile-time limits on upvalues and returned values, at and around each
-- dialect's edge: 5.1 allows 60 upvalues ("has more than 60 upvalues"),
-- later dialects 255; a return statement fails on registers first, except
-- in 5.5 where 255 values fit and the count itself is refused. The
-- " near <token>" suffix is cut: luna places registers after parsing and has
-- no token to name (docs/compatibility.md).
local ld = loadstring or load
local function try(k, src)
  local f, e = ld(src, "=c")
  print(k, f and "ok" or (e:gsub(" near .*$", "")))
end
local function upvalues(n)
  local d, b = {}, {}
  for i = 1, n do d[i] = "local u" .. i .. " = " .. i; b[i] = "s = s + u" .. i end
  return "local function f() " .. table.concat(d, " ")
    .. " return function() local s = 0 " .. table.concat(b, " ") .. " return s end end"
end
for _, n in ipairs({59, 60, 61}) do try("upvalues " .. n, upvalues(n)) end
local function returns(n, pre)
  return (pre or "") .. "return 10" .. string.rep(",10", n - 1)
end
for _, n in ipairs({254, 255, 256, 300}) do
  try("returns " .. n, returns(n))
  try("returns " .. n .. " after a local", returns(n, "local a = 1 "))
end
