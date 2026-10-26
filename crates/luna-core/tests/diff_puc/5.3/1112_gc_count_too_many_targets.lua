-- an assignment with more targets than the parser's C levels allow: what
-- PUC's parser made of the statement up to the error stays as garbage in
-- the count
local function run(src)
  collectgarbage()
  collectgarbage("stop")
  local before = collectgarbage("count") * 1024
  local _, f, msg = pcall(loadstring or load, src, "=many")
  local after = collectgarbage("count") * 1024
  collectgarbage("restart")
  print(f, msg, after - before)
end
run("a" .. string.rep(",a", 300) .. " = 1")
run("local t = {}\nt.x" .. string.rep(", t.x", 250) .. " = 1, 2")
run("local function f() return 1 end\nb" .. string.rep(", b", 199) .. " = f()")
