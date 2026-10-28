-- the line of each instruction run, and where functions are defined, when
-- their tokens are spread over lines: a call and an operator's operand
-- before a binary operator are coded on their own lines, a function
-- expression or a local function is defined on the line of the token after
-- `function` (or after its name), a numeric loop's back jump is on the
-- line of `for`, and a function statement's store on the line of
-- `function`
local load = loadstring or load
local srcs = {
  "local function g(x) return x end\nlocal a = 1\nlocal z = g (\na) + 1",
  "local function g(x) return x end\nlocal a = 1\nlocal z = g ( a ) \n+ 1",
  "local t = {1}\nlocal z = #\nt + 1",
  "local t = {1}\nlocal z = - \nt[1] * 2",
  "s = {}\nfunction\ns.f() return 1 end",
  "local z\nfor\ni = 1, 2 do z = i end",
  "local f = function\n(x)\nreturn x end\nreturn f(1)",
  "local function\nh\n() return 1\nend\nreturn h()",
}
for i, src in ipairs(srcs) do
  local f = assert(load(src, "=far"))
  local ls = {}
  debug.sethook(function() ls[#ls + 1] = debug.getinfo(2, "l").currentline end, "", 1)
  pcall(f)
  debug.sethook()
  print(i, table.concat(ls, " "))
end
local h, g = load("local function\nh\n() end\nlocal g = function\n\n() end\nreturn h, g")()
print(debug.getinfo(h, "S").linedefined, debug.getinfo(g, "S").linedefined)
