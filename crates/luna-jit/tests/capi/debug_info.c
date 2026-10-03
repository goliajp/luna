/* lua_getstack and lua_getinfo: every option of each dialect, on levels
   of the running thread and of a suspended coroutine, and on function
   values ('>') */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#define S(p) ((p) ? (p) : "(null)")

/* the first characters of a string, one line */
static const char *head(const char *s) {
  static char buf[4][28];
  static int k;
  char *b = buf[k++ & 3];
  int i;
  if (s == NULL) return "(null)";
  for (i = 0; i < 24 && s[i]; i++) b[i] = s[i] == '\n' ? '|' : s[i];
  b[i] = 0;
  return b;
}

static void show(lua_State *L, lua_Debug *ar, const char *opts, int st) {
  printf("  [%s] st=%d", opts, st);
  if (strchr(opts, 'S'))
    printf(" what=%s src=%s short=%s def=%d-%d", S(ar->what), head(ar->source),
           ar->short_src, ar->linedefined, ar->lastlinedefined);
  if (strchr(opts, 'l')) printf(" line=%d", ar->currentline);
  if (strchr(opts, 'n')) printf(" name=%s namewhat=[%s]", S(ar->name), S(ar->namewhat));
  if (strchr(opts, 'u')) {
    printf(" nups=%d", (int)ar->nups);
#if LUA_VERSION_NUM >= 502
    printf(" nparams=%d vararg=%d", (int)ar->nparams, (int)ar->isvararg);
#endif
  }
#if LUA_VERSION_NUM >= 502
  if (strchr(opts, 't')) printf(" tail=%d", (int)ar->istailcall);
#endif
#if LUA_VERSION_NUM >= 505
  if (strchr(opts, 't')) printf(" extra=%d", (int)ar->extraargs);
#endif
#if LUA_VERSION_NUM >= 504
  if (strchr(opts, 'r')) printf(" ftr=%d ntr=%d", (int)ar->ftransfer, (int)ar->ntransfer);
#endif
  printf("\n");
  (void)L;
}

#if LUA_VERSION_NUM >= 504
#define ALL "nSltur"
#elif LUA_VERSION_NUM >= 502
#define ALL "nSltu"
#else
#define ALL "nSlu"
#endif

/* walk every level of the running thread */
static int walk(lua_State *L) {
  lua_Debug ar;
  int level = 0, top = lua_gettop(L);
  printf("walk %s\n", lua_tostring(L, 1));
  while (lua_getstack(L, level, &ar)) {
    int st = lua_getinfo(L, ALL, &ar);
    printf(" level %d:", level);
    show(L, &ar, ALL, st);
    level++;
  }
  printf(" levels=%d top=%d\n", level, lua_gettop(L) - top);
  return 0;
}

/* 'f' and 'L' push the function and its lines, in that order */
static int pushes(lua_State *L) {
  lua_Debug ar;
  int lvl = (int)lua_tointeger(L, 1);
  int top = lua_gettop(L);
  if (!lua_getstack(L, lvl, &ar)) { printf("no level %d\n", lvl); return 0; }
  lua_getinfo(L, "Lf", &ar);
  printf("pushes level %d: +%d f=%s L=%s\n", lvl, lua_gettop(L) - top,
         lua_typename(L, lua_type(L, -2)), lua_typename(L, lua_type(L, -1)));
  if (lua_istable(L, -1)) {
    int n;
    lua_setglobal(L, "LINES");
    luaL_loadstring(L, "local t = {} for k in pairs(LINES) do t[#t+1] = k end "
                       "table.sort(t) return table.concat(t, ',')");
    lua_call(L, 0, 1);
    printf("  lines=%s\n", lua_tostring(L, -1));
    lua_pop(L, 1);
    n = 0; (void)n;
  } else lua_pop(L, 1);
  lua_setglobal(L, "FUNC");
  return 0;
}

/* lua_getinfo with '>' on argument 1 */
static int fninfo(lua_State *L) {
  lua_Debug ar;
  int top, st;
  const char *opts = lua_tostring(L, 2);
  char buf[32];
  lua_pushvalue(L, 1);
  top = lua_gettop(L);
  snprintf(buf, sizeof buf, ">%s", opts);
  memset(&ar, 0, sizeof ar);
  st = lua_getinfo(L, buf, &ar);
  printf("fninfo %s: pop/push %d\n", opts, lua_gettop(L) - top);
  show(L, &ar, opts, st);
  lua_settop(L, 0);
  return 0;
}

/* level info of another thread */
static int coinfo(lua_State *L) {
  lua_State *co = lua_tothread(L, 1);
  lua_Debug ar;
  int level = 0;
  while (lua_getstack(co, level, &ar)) {
    int st = lua_getinfo(co, ALL, &ar);
    printf(" co level %d:", level);
    show(co, &ar, ALL, st);
    level++;
  }
  printf(" co levels=%d\n", level);
  return 0;
}

static int edges(lua_State *L) {
  lua_Debug ar;
  int st;
  printf("getstack(-1)=%d\n", lua_getstack(L, -1, &ar));
#if LUA_VERSION_NUM == 501
  st = lua_getinfo(L, "nSlu", &ar);
  printf("  5.1 negative level:");
  show(L, &ar, "nSlu", st);
#endif
  printf("getstack(100)=%d\n", lua_getstack(L, 100, &ar));
  lua_getstack(L, 0, &ar);
  st = lua_getinfo(L, "Sx", &ar);
  printf("invalid option: st=%d what=%s\n", st, ar.what);
  st = lua_getinfo(L, "t", &ar);
  printf("option t: st=%d\n", st);
  st = lua_getinfo(L, "r", &ar);
  printf("option r: st=%d\n", st);
  st = lua_getinfo(L, "", &ar);
  printf("no options: st=%d\n", st);
  return 0;
}

static int upv(lua_State *L) { (void)L; return 0; }

static const char *script =
  "local load = loadstring or load\n"
  "local f = load([[\n"
  "local walk, pushes = ...\n"
  "local t = {}\n"
  "function t.field(x) walk('field') end\n"
  "function t:method(a, b) walk('method') end\n"
  "local function loc(...) walk('local') return 1 end\n"
  "function glob() walk('global') end\n"
  "t.field(1) t:method(1, 2) loc(1) glob()\n"
  "local function tail() return walk('tail') end\n"
  "local function viatail() return tail() end\n"
  "viatail()\n"
  "pushes(0) pushes(1) pushes(5)\n"
  "for i = 1, 1 do pcall(walk, 'pcall') end\n"
  "setmetatable(t, {__index = function(t, k) walk('index') end})\n"
  "local _ = t.missing\n"
  "local co = coroutine.create(function(a)\n"
  "  local function inner() coroutine.yield() end\n"
  "  inner()\n"
  "end)\n"
  "coroutine.resume(co)\n"
  "return co\n"
  "]], '@debug_info.lua')\n"
  "return f(...)\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_pushcfunction(L, coinfo);
  lua_setglobal(L, "coinfo");
  lua_pushcfunction(L, fninfo);
  lua_setglobal(L, "fninfo");
  lua_pushcfunction(L, edges);
  lua_setglobal(L, "edges");
  luaL_loadstring(L, script);
  lua_pushcfunction(L, walk);
  lua_pushcfunction(L, pushes);
  st = lua_pcall(L, 2, 1, 0);
  printf("script: %d %s\n", st, st ? lua_tostring(L, -1) : "ok");
  lua_setglobal(L, "CO");
  st = luaL_loadstring(L,
    "coinfo(CO)\n"
    "edges()\n"
    "local load = loadstring or load\n"
    "fninfo(print, 'nSlu')\n"
    "fninfo(fninfo, 'nSlu')\n"
    "local g = load('local a, b = ...\\nreturn function(x, y, ...)\\n  return a, b\\nend', '=short')(1, 2)\n"
    "fninfo(g, 'nSlu')\n"
    "fninfo(g, 'SLf')\n"
    "local m = load('return 1', 'a chunk name that is quite long and goes on and on and on to be cut')\n"
    "fninfo(m, 'S')\n"
    "fninfo(load('return 1', '@a/very/long/file/name/that/goes/on/and/on/to/be/cut/at/the/front.lua'), 'S')\n"
    "fninfo(load('return 1', '=an equals name that is quite long and goes on and on and on to be cut'), 'S')\n"
    "fninfo(load('return 1', 'two\\nlines'), 'S')\n");
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  printf("rest: %d %s\n", st, st ? lua_tostring(L, -1) : "ok");
  lua_pushcclosure(L, upv, 0);
  lua_pushinteger(L, 1);
  lua_pushinteger(L, 2);
  lua_pushcclosure(L, upv, 2);
  lua_setglobal(L, "U2");
  lua_setglobal(L, "U0");
  luaL_loadstring(L, "fninfo(U2, 'Su') fninfo(U0, 'u')");
  st = lua_pcall(L, 0, 0, 0);
  printf("cclosure: %d\n", st);
  lua_close(L);
  return 0;
}
