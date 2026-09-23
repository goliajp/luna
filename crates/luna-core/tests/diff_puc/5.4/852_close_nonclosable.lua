-- A to-be-closed variable without __close: PUC names the variable (as
-- luaG_findlocal sees it) and nothing else.
local function e(src)
  print(pcall(load(src, "=c")))
end
e("local x <close> = 42")
e("local a, b <close> = 1, {}")
e("for k in next, {}, nil, 42 do end")
e("local x <close> = false return 'ok'")
