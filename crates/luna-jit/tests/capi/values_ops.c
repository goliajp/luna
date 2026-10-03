/* lua_arith, lua_compare, lua_equal, lua_lessthan, lua_concat and lua_len:
   results per dialect, metamethods, errors and stack effects */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void pv(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  switch (t) {
    case LUA_TNONE: printf("none"); break;
    case LUA_TNIL: printf("nil"); break;
    case LUA_TBOOLEAN: printf(lua_toboolean(L, idx) ? "true" : "false"); break;
    case LUA_TNUMBER:
    case LUA_TSTRING:
      lua_pushvalue(L, idx);
      printf("%s:%s", t == LUA_TNUMBER ? "n" : "s", lua_tostring(L, -1));
      lua_pop(L, 1);
      break;
    default: printf("%s", lua_typename(L, t));
  }
}

static void dostr(lua_State *L, const char *s) {
  if (luaL_loadstring(L, s) != 0 || lua_pcall(L, 0, 0, 0) != 0) {
    printf("lua error: ");
    pv(L, -1);
    printf("\n");
    lua_pop(L, 1);
  }
}

static void prot(lua_State *L, const char *name, lua_CFunction f) {
  int st;
  lua_pushcfunction(L, f);
  st = lua_pcall(L, 0, 0, 0);
  printf("%s: status=%d ", name, st);
  if (st != 0) {
    pv(L, -1);
    lua_pop(L, 1);
  }
  printf(" top=%d\n", lua_gettop(L));
}

/* print the value on top and the stack size, and pop it */
static void res(lua_State *L, const char *what) {
  printf("%s: ", what);
  pv(L, -1);
  printf(" top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
}

/* push the value of a Lua expression */
static void expr(lua_State *L, const char *e) {
  char buf[256];
  snprintf(buf, sizeof buf, "return %s", e);
  if (luaL_loadstring(L, buf) != 0 || lua_pcall(L, 0, 1, 0) != 0) {
    printf("expr error: %s\n", lua_tostring(L, -1));
  }
}

#if LUA_VERSION_NUM >= 502
static const char *const opnames[] = {
#if LUA_VERSION_NUM == 502
  "add", "sub", "mul", "div", "mod", "pow", "unm"
#else
  "add", "sub", "mul", "mod", "pow", "div", "idiv", "band", "bor", "bxor", "shl", "shr",
  "unm", "bnot"
#endif
};
#define NOPS ((int)(sizeof opnames / sizeof opnames[0]))

static int unary(int op) {
#if LUA_VERSION_NUM == 502
  return op == LUA_OPUNM;
#else
  return op == LUA_OPUNM || op == LUA_OPBNOT;
#endif
}

/* every operation on the two values of expressions a and b */
static void all_ops(lua_State *L, const char *a, const char *b) {
  int op;
  for (op = 0; op < NOPS; op++) {
    printf("%s %s %s: ", opnames[op], a, unary(op) ? "" : b);
    lua_pushcfunction(L, c_arith1);
    lua_pushinteger(L, op);
    expr(L, a);
    expr(L, b);
    if (lua_pcall(L, 3, 0, 0) != 0) {
      printf("error ");
      pv(L, -1);
      printf("\n");
      lua_pop(L, 1);
    }
  }
}

/* (op, a, b): print lua_arith on a and b (a alone when unary) and the
   stack size it left */
static int c_arith1(lua_State *L) {
  int op = (int)lua_tointeger(L, 1);
  if (unary(op)) lua_settop(L, 2);
  lua_arith(L, op);
  pv(L, -1);
  printf(" top=%d\n", lua_gettop(L));
  return 0;
}

static int e_arith_nil(lua_State *L) {
  lua_pushinteger(L, 1);
  lua_pushnil(L);
  lua_arith(L, LUA_OPADD);
  return 0;
}

static int e_arith_table(lua_State *L) {
  lua_newtable(L);
  lua_arith(L, LUA_OPUNM);
  return 0;
}

static int e_arith_mm(lua_State *L) {
  lua_getglobal(L, "merr");
  lua_pushinteger(L, 1);
  lua_arith(L, LUA_OPMUL);
  return 0;
}

static int e_compare_mixed(lua_State *L) {
  lua_pushinteger(L, 1);
  lua_pushstring(L, "1");
  lua_compare(L, 1, 2, LUA_OPLT);
  return 0;
}

static int e_compare_tables(lua_State *L) {
  lua_newtable(L);
  lua_newtable(L);
  lua_compare(L, 1, 2, LUA_OPLE);
  return 0;
}

static int e_len_number(lua_State *L) {
  lua_pushinteger(L, 3);
  lua_len(L, -1);
  return 0;
}

static int e_len_nil(lua_State *L) {
  lua_len(L, 5);
  return 0;
}
#endif

#if LUA_VERSION_NUM >= 503
static int e_idiv_zero(lua_State *L) {
  lua_pushinteger(L, 1);
  lua_pushinteger(L, 0);
  lua_arith(L, LUA_OPIDIV);
  return 0;
}

static int e_mod_zero(lua_State *L) {
  lua_pushinteger(L, 1);
  lua_pushinteger(L, 0);
  lua_arith(L, LUA_OPMOD);
  return 0;
}

static int e_band_float(lua_State *L) {
  lua_pushnumber(L, 1.5);
  lua_pushinteger(L, 1);
  lua_arith(L, LUA_OPBAND);
  return 0;
}

static int e_bnot_string(lua_State *L) {
  lua_pushstring(L, "x");
  lua_arith(L, LUA_OPBNOT);
  return 0;
}
#endif

static int e_concat_nil(lua_State *L) {
  lua_pushstring(L, "a");
  lua_pushnil(L);
  lua_concat(L, 2);
  return 0;
}

static int e_concat_table(lua_State *L) {
  lua_newtable(L);
  lua_pushinteger(L, 1);
  lua_pushstring(L, "b");
  lua_concat(L, 3);
  return 0;
}

static int e_concat_mm(lua_State *L) {
  lua_getglobal(L, "cerr");
  lua_pushstring(L, "b");
  lua_concat(L, 2);
  return 0;
}

#if LUA_VERSION_NUM == 501
#define EQ(L, a, b) lua_equal(L, a, b)
#define LT(L, a, b) lua_lessthan(L, a, b)
#else
#define EQ(L, a, b) lua_compare(L, a, b, LUA_OPEQ)
#define LT(L, a, b) lua_compare(L, a, b, LUA_OPLT)
#endif

static int e_lt_mixed(lua_State *L) {
  lua_pushinteger(L, 1);
  lua_pushstring(L, "1");
  LT(L, 1, 2);
  return 0;
}

static int e_lt_mm(lua_State *L) {
  lua_getglobal(L, "lterr");
  lua_pushvalue(L, -1);
  LT(L, 1, 2);
  return 0;
}

/* (a, b): print their comparisons */
static int c_cmp(lua_State *L) {
  printf("eq=%d", EQ(L, 1, 2));
  printf(" lt=%d", LT(L, 1, 2));
#if LUA_VERSION_NUM >= 502
  printf(" le=%d", lua_compare(L, 1, 2, LUA_OPLE));
#endif
  printf(" top=%d\n", lua_gettop(L));
  return 0;
}

/* the comparisons of expressions a and b */
static void cmp(lua_State *L, const char *a, const char *b) {
  printf("cmp %s %s: ", a, b);
  lua_pushcfunction(L, c_cmp);
  expr(L, a);
  expr(L, b);
  if (lua_pcall(L, 2, 0, 0) != 0) {
    printf(" error ");
    pv(L, -1);
    printf("\n");
    lua_pop(L, 1);
  }
}

/* concatenate the values of the expressions in es, n of them */
static void concat(lua_State *L, const char *const *es, int n, const char *what) {
  int i;
  for (i = 0; i < n; i++) expr(L, es[i]);
  lua_concat(L, n);
  res(L, what);
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  dostr(L, "mt = {__add = function(a, b) return 'add' end,"
           " __unm = function(a, b) return 'unm ' .. tostring(rawequal(a, b)) end,"
           " __concat = function(a, b) return 'M' end,"
           " __len = function(a) return 'L' end,"
           " __eq = function(a, b) return true end,"
           " __lt = function(a, b) return rawget(a, 'v') < rawget(b, 'v') end,"
           " __bnot = function(a, b) return 'bnot ' .. tostring(rawequal(a, b)) end,"
           " __idiv = function(a, b) return 'idiv' end}"
           " A = setmetatable({v = 1}, mt) B = setmetatable({v = 2}, mt) P = {v = 3}");

#if LUA_VERSION_NUM >= 502
  all_ops(L, "7", "2");
  all_ops(L, "7.5", "2");
  all_ops(L, "-7", "2");
  all_ops(L, "'10'", "'3'");
  all_ops(L, "0", "0.5");
  all_ops(L, "A", "1");
  lua_pushinteger(L, 5);
  lua_pushinteger(L, 6);
  lua_pushinteger(L, 7);
  lua_arith(L, LUA_OPADD);
  printf("arith keeps the rest: ");
  pv(L, 1);
  printf(" ");
  pv(L, 2);
  printf(" top=%d\n", lua_gettop(L));
  lua_settop(L, 0);
  prot(L, "arith nil", e_arith_nil);
  prot(L, "arith table", e_arith_table);
  dostr(L, "merr = setmetatable({}, {__mul = function() error('mulerr', 0) end})");
  prot(L, "arith mm error", e_arith_mm);
#endif
#if LUA_VERSION_NUM >= 503
  all_ops(L, "3.0", "1");
  prot(L, "idiv zero", e_idiv_zero);
  prot(L, "mod zero", e_mod_zero);
  prot(L, "band float", e_band_float);
  prot(L, "bnot string", e_bnot_string);
#endif

  cmp(L, "1", "2");
  cmp(L, "2", "1");
  cmp(L, "1", "1.0");
  cmp(L, "'a'", "'b'");
  cmp(L, "'b'", "'a'");
  cmp(L, "'a'", "'a'");
  cmp(L, "A", "B");
  cmp(L, "B", "A");
  cmp(L, "A", "A");
  cmp(L, "A", "P");
  cmp(L, "{}", "{}");
  cmp(L, "nil", "false");
  cmp(L, "0/0", "0/0");
  lua_pushinteger(L, 1);
  printf("cmp invalid: eq=%d lt=%d top=%d\n", EQ(L, 1, 5), LT(L, 5, 1), lua_gettop(L));
  lua_settop(L, 0);
  prot(L, "lt mixed", e_lt_mixed);
  dostr(L, "lterr = setmetatable({}, {__lt = function() error('lterr', 0) end})");
  prot(L, "lt mm error", e_lt_mm);
#if LUA_VERSION_NUM >= 502
  prot(L, "compare mixed", e_compare_mixed);
  prot(L, "compare tables le", e_compare_tables);
  /* __le falls back to not __lt(b, a) up to 5.3 */
  dostr(L, "print('le fallback', pcall(function() return A <= B end))");
  lua_settop(L, 0);
#endif

  {
    static const char *const two[] = {"'a'", "1"};
    static const char *const three[] = {"'x'", "2.5", "'y'"};
    static const char *const ints[] = {"1", "2"};
    static const char *const mixed[] = {"'a'", "A", "'b'", "'c'"};
    static const char *const mmleft[] = {"A", "'s'"};
    static const char *const nums[] = {"1", "1.0", "-0.0"};
    concat(L, two, 2, "concat a 1");
    concat(L, three, 3, "concat x 2.5 y");
    concat(L, ints, 2, "concat 1 2");
    concat(L, mixed, 4, "concat a A b c");
    concat(L, mmleft, 2, "concat A s");
    concat(L, nums, 3, "concat 1 1.0 -0.0");
  }
  lua_concat(L, 0);
  res(L, "concat 0");
  lua_pushinteger(L, 3);
  lua_concat(L, 1);
  printf("concat 1: %s ", luaL_typename(L, -1));
  res(L, "");
  lua_pushstring(L, "below");
  lua_pushstring(L, "p");
  lua_pushstring(L, "q");
  lua_concat(L, 2);
  printf("concat keeps the rest: ");
  pv(L, 1);
  printf(" ");
  pv(L, 2);
  printf(" top=%d\n", lua_gettop(L));
  lua_settop(L, 0);
  dostr(L, "cgc = setmetatable({}, {__concat = function(a, b)"
           " collectgarbage() collectgarbage() return #b end})");
  lua_getglobal(L, "cgc");
  lua_pushstring(L, "a string that is quite a bit longer than forty bytes");
  lua_pushstring(L, " and another one that is also long enough to matter");
  lua_concat(L, 3);
  res(L, "concat gc");
  prot(L, "concat nil", e_concat_nil);
  prot(L, "concat table", e_concat_table);
  dostr(L, "cerr = setmetatable({}, {__concat = function() error('caterr', 0) end})");
  prot(L, "concat mm error", e_concat_mm);

#if LUA_VERSION_NUM >= 502
  lua_pushstring(L, "hello");
  lua_len(L, -1);
  res(L, "len string");
  lua_pop(L, 1);
  expr(L, "{1, 2, 3}");
  lua_len(L, -1);
  res(L, "len table");
  lua_pop(L, 1);
  expr(L, "A");
  lua_len(L, 1);
  res(L, "len __len");
  lua_pop(L, 1);
  lua_newtable(L);
  lua_len(L, LUA_REGISTRYINDEX);
  printf("len registry: %s\n", luaL_typename(L, -1));
  lua_settop(L, 0);
  prot(L, "len number", e_len_number);
  prot(L, "len nil", e_len_nil);
#endif

  printf("final top=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
