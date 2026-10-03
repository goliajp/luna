/* Argument checks and error reports: luaL_check*, luaL_opt*, luaL_argerror
   and the type errors, luaL_where and luaL_error, called from C and from
   Lua, as functions and as methods */
#include "aux_common.h"

static int f_checkinteger(lua_State *L) {
  lua_pushinteger(L, luaL_checkinteger(L, 1));
  return 1;
}

static int f_checknumber(lua_State *L) {
  lua_pushnumber(L, luaL_checknumber(L, 1));
  return 1;
}

static int f_checkstring(lua_State *L) {
  size_t len = 0;
  const char *s = luaL_checklstring(L, 1, &len);
  lua_pushlstring(L, s, len);
  lua_pushinteger(L, (lua_Integer)len);
  return 2;
}

static int f_opts(lua_State *L) {
  size_t len = 99;
  const char *s = luaL_optlstring(L, 1, "dflt", &len);
  lua_pushstring(L, s == NULL ? "(null)" : s);
  lua_pushinteger(L, (lua_Integer)len);
  lua_pushinteger(L, luaL_optinteger(L, 2, -5));
  lua_pushnumber(L, luaL_optnumber(L, 3, 0.5));
  s = luaL_optlstring(L, 4, NULL, &len);
  lua_pushboolean(L, s == NULL);
  lua_pushinteger(L, (lua_Integer)len);
  return 6;
}

static int f_checktype(lua_State *L) {
  luaL_checktype(L, 1, LUA_TTABLE);
  lua_pushstring(L, "ok");
  return 1;
}

static int f_checkany(lua_State *L) {
  luaL_checkany(L, 2);
  lua_pushstring(L, "ok");
  return 1;
}

static int f_checkoption(lua_State *L) {
  static const char *const opts[] = {"alpha", "beta", "gamma", NULL};
  lua_pushinteger(L, luaL_checkoption(L, 1, NULL, opts));
  lua_pushinteger(L, luaL_checkoption(L, 2, "gamma", opts));
  return 2;
}

static int f_argerror(lua_State *L) {
  return luaL_argerror(L, (int)luaL_optinteger(L, 1, 1), "custom message");
}

static int f_typeerror(lua_State *L) {
#if LUA_VERSION_NUM == 501
  return luaL_typerror(L, 1, "widget");
#elif LUA_VERSION_NUM >= 504
  return luaL_typeerror(L, 1, "widget");
#else
  return luaL_argerror(L, 1, lua_pushfstring(L, "widget expected, got %s", luaL_typename(L, 1)));
#endif
}

static int f_error(lua_State *L) {
  return luaL_error(L, "fail %d %s", 3, "x");
}

static int f_where(lua_State *L) {
  int lvl = (int)luaL_checkinteger(L, 1);
  luaL_where(L, lvl);
  return 1;
}

static int f_checkstack(lua_State *L) {
  luaL_checkstack(L, 50, "fifty");
  lua_pushstring(L, "small ok");
  luaL_checkstack(L, 100000000, lua_gettop(L) > 100 ? NULL : "too many");
  lua_pushstring(L, "big ok");
  return 2;
}

static int f_checkstack_null(lua_State *L) {
  luaL_checkstack(L, 100000000, NULL);
  return 0;
}

#if LUA_VERSION_NUM == 502
static int f_unsigned(lua_State *L) {
  lua_pushnumber(L, (lua_Number)luaL_checkunsigned(L, 1));
  lua_pushnumber(L, (lua_Number)luaL_optunsigned(L, 2, 77));
  return 2;
}
#endif

static const char *const checks =
    "local t = {}\n"
    "for _, name in ipairs{'ci', 'cn', 'cs', 'ct', 'ae', 'te', 'er'} do\n"
    "  t[name] = _G[name]\n"
    "end\n"
    "local function try(label, f, ...)\n"
    "  print(label, pcall(f, ...))\n"
    "end\n"
    "try('global ci', function() return ci('x') end)\n"
    "try('local ci', function() local f = ci; return f({}) end)\n"
    "try('method ci', function() return t:ci() end)\n"
    "try('method arg', function() return t:cn('a') end)\n"
    "try('field cs', function() return t.cs({}) end)\n"
    "try('field ct', function() return t.ct(1) end)\n"
    "try('direct ae', ae, 3)\n"
    "try('method ae self', function() return t:ae(1) end)\n"
    "try('method ae 2', function() return t:ae(2) end)\n"
    "try('te', function() return te(nil) end)\n"
    "try('te ud', function() return te(io.stdout) end)\n"
    "try('er', function() er() end)\n"
    "try('er direct', er)\n";

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  run(L, "checkinteger 7", f_checkinteger, "return 7");
  run(L, "checkinteger '8'", f_checkinteger, "return '8'");
  run(L, "checkinteger 2.0", f_checkinteger, "return 2.0");
  run(L, "checkinteger 2.5", f_checkinteger, "return 2.5");
  run(L, "checkinteger '2.5'", f_checkinteger, "return '2.5'");
  run(L, "checkinteger nil", f_checkinteger, "return nil");
  run(L, "checkinteger none", f_checkinteger, NULL);
  run(L, "checkinteger table", f_checkinteger, "return {}");
  run(L, "checkinteger huge", f_checkinteger, "return 2^63");
  run(L, "checknumber 1.5", f_checknumber, "return 1.5");
  run(L, "checknumber ' 0x10 '", f_checknumber, "return ' 0x10 '");
  run(L, "checknumber 'z'", f_checknumber, "return 'z'");
  run(L, "checknumber true", f_checknumber, "return true");
  run(L, "checkstring", f_checkstring, "return 'a\\0b'");
  run(L, "checkstring number", f_checkstring, "return 12");
  run(L, "checkstring bool", f_checkstring, "return false");
  run(L, "opts none", f_opts, NULL);
  run(L, "opts nils", f_opts, "return nil, nil, nil, nil");
  run(L, "opts given", f_opts, "return 'abc', 9, 2.25, 'x'");
  run(L, "opts bad int", f_opts, "return nil, 'q'");
  run(L, "opts bad num", f_opts, "return nil, nil, {}");
  run(L, "checktype ok", f_checktype, "return {}");
  run(L, "checktype bad", f_checktype, "return 'no'");
  run(L, "checktype none", f_checktype, NULL);
  run(L, "checkany ok", f_checkany, "return 1, nil");
  run(L, "checkany none", f_checkany, "return 1");
  run(L, "checkoption ok", f_checkoption, "return 'beta'");
  run(L, "checkoption default", f_checkoption, "return 'alpha', nil");
  run(L, "checkoption bad", f_checkoption, "return 'delta'");
  run(L, "checkoption bad default", f_checkoption, "return 'alpha', 'zeta'");
  run(L, "checkoption missing", f_checkoption, NULL);
  run(L, "argerror", f_argerror, "return 2");
  run(L, "argerror 0", f_argerror, "return 0");
  run(L, "typeerror nil", f_typeerror, "return nil");
  run(L, "typeerror light", f_typeerror, NULL);
  run(L, "error", f_error, NULL);
  run(L, "where 0", f_where, "return 0");
  run(L, "where 1", f_where, "return 1");
  run(L, "where 5", f_where, "return 5");
  run(L, "checkstack", f_checkstack, NULL);
  run(L, "checkstack null", f_checkstack_null, NULL);
#if LUA_VERSION_NUM == 502
  run(L, "unsigned", f_unsigned, "return 5");
  run(L, "unsigned opt", f_unsigned, "return 3, 4");
  run(L, "unsigned bad", f_unsigned, "return 'w'");
#endif
  /* from Lua: names come from the call */
  lua_register(L, "ci", f_checkinteger);
  lua_register(L, "cn", f_checknumber);
  lua_register(L, "cs", f_checkstring);
  lua_register(L, "ct", f_checktype);
  lua_register(L, "ae", f_argerror);
  lua_register(L, "te", f_typeerror);
  lua_register(L, "er", f_error);
  dochunk(L, checks);
  lua_register(L, "wh", f_where);
  dochunk(L, "local function f() return wh(1) end\n"
             "print('where Lua', f())\n"
             "print('where C', wh(0))\n"
             "print('where main', wh(2))");
  /* a light userdata argument */
  lua_settop(L, 0);
  lua_pushcfunction(L, f_typeerror);
  lua_pushlightuserdata(L, (void *)L);
  printf("typeerror light: status=%d ", lua_pcall(L, 1, 0, 0));
  put_value(L, -1);
  printf("\n");
  /* a userdata with __name */
  lua_settop(L, 0);
  lua_pushcfunction(L, f_checkinteger);
  lua_newuserdata(L, 4);
  lua_newtable(L);
  lua_pushstring(L, "MyType");
  lua_setfield(L, -2, "__name");
  lua_setmetatable(L, -2);
  printf("named userdata: status=%d ", lua_pcall(L, 1, 0, 0));
  put_value(L, -1);
  printf("\n");
  lua_settop(L, 0);
  printf("top at end=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
