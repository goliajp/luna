/* coroutines suspended inside C functions and resumed from Lua
   (coroutine.resume and coroutine.wrap), many times over so that the Lua
   code around them gets hot; 5.1's `return lua_yield(L, n)` */
#include "threads_common.h"

/* yields its argument; the resume's values are its results */
static int c_yield(lua_State *L) {
  lua_pushinteger(L, lua_tointeger(L, 1) * 2);
  return lua_yield(L, 1);
}

/* yields only on every third call */
static int c_maybe_yield(lua_State *L) {
  lua_Integer i = lua_tointeger(L, 1);
  if (i % 3 == 0) return lua_yield(L, 1);
  lua_pushinteger(L, -i);
  return 1;
}

#if LUA_VERSION_NUM >= 502
/* the continuation doubles what the resume passed */
KDEF(k_double) {
  KARGS;
  (void)ctx;
  if (status != LUA_YIELD) printf("k_double: status %d\n", status);
  lua_pushinteger(L, lua_tointeger(L, -1) * 2 + (lua_Integer)ctx);
  return 1;
}

static int c_yieldk(lua_State *L) {
  lua_pushvalue(L, 1);
  return lua_yieldk(L, 1, 1, k_double);
}

/* calls its argument (a function that may yield) and adds one */
KDEF(k_add) {
  KARGS;
  (void)status;
  (void)ctx;
  lua_pushinteger(L, lua_tointeger(L, -1) + 1);
  return 1;
}

static int c_callk(lua_State *L) {
  lua_pushvalue(L, 1);
  lua_pushvalue(L, 2);
  lua_callk(L, 1, 1, 0, k_add);
  return k_add(L
#if LUA_VERSION_NUM >= 503
               , LUA_OK, 0
#endif
  );
}

/* protected call of its argument; the result is the status and the value */
KDEF(k_status) {
  KARGS;
  (void)ctx;
  lua_pushinteger(L, status == LUA_YIELD ? 0 : status);
  lua_insert(L, -2);
  return 2;
}

static int c_pcallk(lua_State *L) {
  int st;
  lua_pushvalue(L, 1);
  lua_pushvalue(L, 2);
  st = lua_pcallk(L, 1, 1, 0, 0, k_status);
  lua_pushinteger(L, st);
  lua_insert(L, -2);
  return 2;
}
#endif

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  reg(L, "c_yield", c_yield);
  reg(L, "c_maybe_yield", c_maybe_yield);
  run(L, "local co = coroutine.create(function(n) local s = 0\n"
         "  for i = 1, n do s = s + c_yield(i) end\n"
         "  return s end)\n"
         "local ok, v = coroutine.resume(co, 2000)\n"
         "local got = 0\n"
         "while coroutine.status(co) == 'suspended' do got = got + v; ok, v = coroutine.resume(co, v + 1) end\n"
         "print('c_yield', ok, v, got)");
  run(L, "local f = coroutine.wrap(function() local s = 0\n"
         "  for i = 1, 3000 do s = s + c_maybe_yield(i) end\n"
         "  return 'end', s end)\n"
         "local n, last = 0, nil\n"
         "while true do local a, b = f(n) if a == 'end' then last = b break end n = n + 1 end\n"
         "print('c_maybe_yield', n, last)");
  run(L, "local co = coroutine.wrap(function(...) return c_yield(...) end)\n"
         "print('wrap first', co(21)) print('wrap second', co('a', 'b'))\n"
         "print('wrap dead', pcall(co))");
#if LUA_VERSION_NUM >= 502
  reg(L, "c_yieldk", c_yieldk);
  reg(L, "c_callk", c_callk);
  reg(L, "c_pcallk", c_pcallk);
  run(L, "local co = coroutine.wrap(c_yieldk)\n"
         "print('c body', co(5)) print('c body ret', co(7)) print('c body dead', pcall(co))");
  run(L, "local co = coroutine.wrap(function() local s = 0\n"
         "  for i = 1, 3000 do s = s + c_yieldk(i) end\n"
         "  return 'end', s end)\n"
         "local v, total = co(), 0\n"
         "while v ~= 'end' do total = total + v; v = co(v) end\n"
         "print('c_yieldk', total)");
  run(L, "local function y(x) return coroutine.yield(x) + x end\n"
         "local co = coroutine.wrap(function() local s = 0\n"
         "  for i = 1, 3000 do s = s + c_callk(y, i) end\n"
         "  return 'end', s end)\n"
         "local v, total = co(), 0\n"
         "while v ~= 'end' do total = total + v; v = co(v * 3) end\n"
         "print('c_callk', total)");
  run(L, "local function y(x) local r = coroutine.yield(x); if r % 7 == 0 then error('bad ' .. r, 0) end; return r end\n"
         "local co = coroutine.wrap(function() local s, errs = 0, 0\n"
         "  for i = 1, 3000 do local st, v = c_pcallk(y, i); if st == 0 then s = s + v else errs = errs + 1 end end\n"
         "  return 'end', s, errs end)\n"
         "local v, s, e = co()\n"
         "while v ~= 'end' do v, s, e = co(v) end\n"
         "print('c_pcallk', s, e)");
  run(L, "local co = coroutine.create(function() return c_yieldk(1) end)\n"
         "print('status', coroutine.resume(co))\n"
         "print('status', coroutine.status(co), coroutine.resume(co, 10))\n"
         "print('status', coroutine.status(co), coroutine.resume(co))");
#endif
#if LUA_VERSION_NUM >= 504
  run(L, "local co = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() print('closed') end})\n"
         "  c_yield(1) end)\n"
         "coroutine.resume(co)\n"
         "print('close', coroutine.close(co), coroutine.status(co))");
#endif
  lua_close(L);
  return 0;
}
