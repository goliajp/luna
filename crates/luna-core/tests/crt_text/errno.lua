-- What each operation leaves in the C library's errno, read through a
-- failure that sets none: in the MSVC C library a write right after a read
-- (5.1-5.3; 5.4 and later clear errno first). File names are under the
-- global DIR when it is set.
local D = DIR or ""
function P(n) return D .. n end
io.stdout:setvbuf("no")
local load = loadstring or load
local function put(name, s) local f = assert(io.open(name, "wb")); f:write(s); f:close() end
put(P("ro.txt"), "aa\n")
local function set9() local f = io.open(P("ro.txt"), "r"); f:write("X"); f:close() end   -- EBADF
local function set22() local f = io.open(P("ro.txt"), "r"); f:seek("set", -1); f:close() end -- EINVAL
local function readerrno()
  put(P("rw.txt"), "aa\nbb\n")
  local f = io.open(P("rw.txt"), "r+"); f:read("*l")
  local a, b, c = f:write("X"); f:close()
  return a and "ok" or tostring(c)
end
local OPS = {
  "local x = tonumber('1e999')", "local x = tonumber('-1e999')", "local x = tonumber('1e-999')",
  "local x = tonumber('4e-320')", "local x = tonumber('2.2250738585072011e-308')", "local x = tonumber('1.5')",
  "local x = tonumber('0x1p-1080')", "local x = tonumber('0x1p2000')", "local x = tonumber('0x10')",
  "local x = tonumber('0xffffffffffffffffff')", "local x = tonumber('99999999999999999999')",
  "local x = tonumber('1e999', 10)", "local x = tonumber('zz', 36)", "local x = tonumber('nan')", "local x = tonumber('inf')",
  "local x = '1e999' + 0", "local x = '1e-999' * 1", "local x = 1e999", "local x = 1e-999", "local x = 0x1p2000",
  "local x = 10 ^ 400", "local a = 10 local x = a ^ 400", "local a = 0 local x = a ^ -1", "local a = -1 local x = a ^ 0.5",
  "local a = 1 local x = a % 0", "local a = 1.5 local x = a % 0", "local a = math.huge local x = a % 2",
  "local x = 5 % 0", "local x = 1 / 0", "local x = -(0/0)",
  "local x = math.log(0)", "local x = math.log(-1)", "local x = math.log10(0)", "local x = math.log(0, 2)",
  "local x = math.log(0, 10)", "local x = math.log(8, 2)", "local x = math.exp(1000)", "local x = math.exp(-1000)",
  "local x = math.sqrt(-1)", "local x = math.acos(2)", "local x = math.asin(2)", "local x = math.atan(1, 0)",
  "local x = math.sin(math.huge)", "local x = math.cos(math.huge)", "local x = math.tan(math.huge)",
  "local x = math.fmod(1, 0.0)", "local x = math.fmod(math.huge, 1)", "local x = math.fmod(1, 0)",
  "local x = math.floor(1e300)", "local x = math.ceil(-0.5)", "local x = math.abs(-1)",
  "local x = math.pow and math.pow(10, 400)", "local x = math.ldexp and math.ldexp(1, 5000)",
  "local x = math.ldexp and math.ldexp(1, -5000)", "local x = math.frexp and math.frexp(0)",
  "local x = math.sinh and math.sinh(1000)", "local x = math.cosh and math.cosh(1000)", "local x = math.tanh and math.tanh(1000)",
  "local x = math.log(0/0)", "local x = math.exp(0/0)", "local x = math.sqrt(0/0)", "local x = math.acos(0/0)",
  "local x = math.fmod(0/0, 0)", "local a = 0/0 local x = a ^ 2.5", "local a = -1 local x = a ^ (0/0)",
  "local x = math.exp(-700)", "local x = math.exp(-745)", "local a = 10 local x = a ^ -320", "local a = 2 local x = a ^ -1074",
  "local x = math.ldexp and math.ldexp(1, -1074)", "local x = math.ldexp and math.ldexp(1, 1024)",
  "local x = tonumber('0x1p-1074')", "local x = tonumber('4.9406564584124654e-324')", "local x = tonumber('0x1.8p-1074')",
  "local x = tonumber('0x' .. string.rep('f', 300))", "local x = tonumber('1' .. string.rep('0', 400))",
  "local x = tonumber(' 0x1p2000 ')", "local x = tonumber('0x1p2000z')", "local x = tonumber('1e999x')",
  "local x = '0x1p2000' + 0", "local x = math.floor('1e999')", "local x = ('1e999'):rep(1) + 1",
  "local x = string.format('%f', '1e999')", "local x = ('x'):rep('1e999' and 1)", "local x = #('1e999' .. '')",
  "local x = tostring('1e999' + 0)", "local x = math.max('1e999', 1)",
  "local x = math.modf(1.5)", "local x = math.random()", "local x = math.tointeger and math.tointeger(2^63)",
  "local x = string.format('%d', 3)", "local x = string.format('%5.2f', 1e300)", "local x = string.format('%a', 1)",
  "local x = string.rep('x', 3)", "local x = ('%g'):format(1e999)", "local x = tostring(1e300)",
  "local x = io.open(P('no/such/file'))", "local x = io.open(P('.'))", "local f = io.open(P('t.txt'), 'w') f:close()",
  "local f = io.open(P('t.txt'), 'r') f:close()", "local f = io.open(P('t.txt'), 'a+') f:close()",
"local x = os.remove(P('no-such'))", "local x = os.rename(P('no-a'), P('no-b'))",
  "local f = io.open(P('t1.txt'), 'w') f:close() os.rename(P('t1.txt'), P('t2.txt')) os.remove(P('t2.txt'))",
  "local x = os.tmpname() os.remove(x)", "local x = io.tmpfile() x:close()",
  "local x = os.execute('exit 3')", "local x = os.execute()", "local p = io.popen('exit 3') p:close()",
  "local p = io.popen('echo hi') p:read('*a') p:close()",
  "local x = os.time()", "local x = os.time({year = 2000, month = 1, day = 1})",
  "local x = os.time({year = 2000, month = 1, day = 1, isdst = false})",
  "local x = os.time({year = 1e9, month = 1, day = 1})", "local x = os.date()", "local x = os.date('%Y', 0)",
  "local x = os.date('*t')", "local x = os.date('!*t', 0)", "local x = os.clock()", "local x = os.getenv('PATH')",
  "local x = os.getenv('NO_SUCH_VAR')", "local x = os.difftime(1, 0)", "local x = os.setlocale()",
  "local x = os.setlocale('xx_YY')", "local x = os.setlocale('C')",
  "local x = pcall(require, 'no_such_module')", "local x = loadfile(P('no-such.lua'))", "local x = pcall(dofile, P('no-such.lua'))",
  "local x = io.lines and pcall(io.lines, P('no-such'))",
  "local f = io.open(P('ro.txt'), 'r') f:read('*n') f:close()",
  "local f = io.open(P('t.txt'), 'w') f:write('1e999') f:close() f = io.open(P('t.txt'), 'r') f:read('*n') f:close()",
  "local f = io.open(P('t.txt'), 'w') f:write('0x1p2000') f:close() f = io.open(P('t.txt'), 'r') f:read('*n') f:close()",
  "local f = io.open(P('t.txt'), 'w') f:write('x') f:close() f = io.open(P('t.txt'), 'r') f:read('*n') f:close()",
  "local f = io.open(P('ro.txt'), 'r') f:read('*a') f:read('*a') f:close()",
  "local f = io.open(P('ro.txt'), 'r') f:seek('end') f:close()", "local f = io.open(P('ro.txt'), 'r') f:setvbuf('no') f:close()",
  "local f = io.open(P('t.txt'), 'w') f:write(1e300, 1) f:flush() f:close()",
  "print('')", "io.write('')", "io.stdout:write('')", "io.stderr:write('')", "local x = io.read and io.type(io.stdin)",
  "collectgarbage()", "local x = select('#', 1, 2)", "local t = {} for i = 1, 100 do t[i] = i end",
  "local x = load('return 1e999')", "local x = load('return 0x1p-1080')", "local x = load('x = ')",
  "local x = string.dump and string.dump(function() end)",
  "local f = io.open(P('t.txt'), 'w') f:close() f = io.open(P('t.txt'), 'w+') f:close()",
  "local f = io.open(P('t.txt'), 'w') f:write('x') f:close() f = io.open(P('t.txt'), 'r+') f:close()",
  "local f = io.open(P('t.txt'), 'w') f:close() f = io.open(P('t.txt'), 'r+b') f:close()",
  "local f = io.open(P('t.txt'), 'w') f:write('x\\26') f:close() f = io.open(P('t.txt'), 'a+') f:close()",
}
for i, op in ipairs(OPS) do
  local f, err = load(op)
  local a, b
  if f then
    set9(); local ok, e = pcall(f); a = readerrno()
    set22(); f = load(op); pcall(f); b = readerrno()
    print(i, a, b, op, ok and "" or ("error " .. tostring(e)))
  else
    print(i, "load error", op, err)
  end
end
