-- 5.1 has no _ENV: the name is an ordinary global, and a local of that name
-- does not redirect global access
print(_ENV)
_ENV = {}
print(type(_ENV), rawget(_G, "_ENV") == _ENV, getfenv(1)._ENV == _ENV)
local function f() return print ~= nil, x end
x = 5
print(f())
local g = function() _ENV = 7 end
g()
print(_ENV, f())
do
  local _ENV = { x = 1 }
  print(x, _ENV.x, type(print))
  local function h() return _ENV.x, x end
  print(h())
  local name, v = debug.getupvalue(h, 1)
  print(name, type(v), debug.getupvalue(h, 2))
  local ok, err = pcall(function() _ENV.nope() end)
  print(ok, (string.gsub(err, "^[^:]*:%d+: ", "")))
end
local function p(_ENV) return _ENV, x end
print(p(3))
setfenv(f, { x = "env", print = print })
print(f())
print(_ENV, x)
