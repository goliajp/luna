/* The standard libraries one at a time: what each luaopen_* returns and
   leaves behind (globals, package.loaded) in a fresh state, called
   directly and through lua_call; luaL_requiref, luaL_openlibs,
   luaL_getsubtable, luaL_setfuncs, and 5.1/5.2's module functions */
#include "aux_common.h"

struct lib {
  const char *name;
  lua_CFunction f;
};

static const struct lib libs[] = {
    {"_G", luaopen_base},       {"package", luaopen_package}, {"table", luaopen_table},
    {"io", luaopen_io},         {"os", luaopen_os},           {"string", luaopen_string},
    {"math", luaopen_math},     {"debug", luaopen_debug},
#if LUA_VERSION_NUM >= 502
    {"coroutine", luaopen_coroutine},
#endif
#if LUA_VERSION_NUM == 502 || LUA_VERSION_NUM == 503
    {"bit32", luaopen_bit32},
#endif
#if LUA_VERSION_NUM >= 503
    {"utf8", luaopen_utf8},
#endif
    {NULL, NULL}};

static const char *const globals[] = {"_G", "package", "coroutine", "table", "io",
                                      "os", "string", "bit32", "math", "utf8",
                                      "debug", "require", "module", "print",
                                      "loadfile", "unpack", NULL};

/* which of the names are globals, and which are in package.loaded */
static void census(lua_State *L, const char *label) {
  int i;
  printf("  %s globals:", label);
  for (i = 0; globals[i]; i++) {
    lua_getglobal(L, globals[i]);
    if (!lua_isnil(L, -1)) printf(" %s", globals[i]);
    lua_pop(L, 1);
  }
  printf("\n  %s loaded:", label);
  lua_getfield(L, LUA_REGISTRYINDEX, "_LOADED");
  if (lua_istable(L, -1)) {
    for (i = 0; globals[i]; i++) {
      lua_getfield(L, -1, globals[i]);
      if (!lua_isnil(L, -1)) printf(" %s", globals[i]);
      lua_pop(L, 1);
    }
  }
  else
    printf(" (no _LOADED)");
  lua_pop(L, 1);
  printf("\n");
}

/* the returned values: types, and whether they are the global of the
   library's name */
static void returned(lua_State *L, const char *name, int n, int base) {
  int i;
  printf("  returned %d (top %d):", n, lua_gettop(L) - base);
  for (i = lua_gettop(L) - n + 1; i <= lua_gettop(L); i++) {
    printf(" %s", luaL_typename(L, i));
    lua_getglobal(L, name);
    if (lua_rawequal(L, -1, i)) printf("=global");
    lua_pop(L, 1);
  }
  printf("\n");
}

static void open_direct(const struct lib *lib) {
  lua_State *L = luaL_newstate();
  int n;
  lua_pushstring(L, "below");
  n = lib->f(L);
  printf("%s direct:\n", lib->name);
  returned(L, lib->name, n, 1);
  census(L, lib->name);
  if (strcmp(lib->name, "string") == 0) {
    int st = luaL_dostring(L, "return ('x'):rep(3)");
    printf("  string methods: %d %s\n", st, lua_tostring(L, -1));
    lua_settop(L, 0);
  }
  lua_close(L);
}

static void open_call(const struct lib *lib) {
  lua_State *L = luaL_newstate();
  int st;
  lua_pushcfunction(L, lib->f);
  lua_pushstring(L, lib->name);
  st = lua_pcall(L, 1, LUA_MULTRET, 0);
  printf("%s call: status=%d results=%d\n", lib->name, st, lua_gettop(L));
  census(L, lib->name);
  lua_close(L);
}

static int f_a(lua_State *L) {
  lua_pushstring(L, "a");
  lua_pushvalue(L, lua_upvalueindex(1));
  return 2;
}

static int f_b(lua_State *L) {
  lua_pushstring(L, "b");
  return 1;
}

static const luaL_Reg funcs[] = {{"a", f_a}, {"b", f_b}, {NULL, NULL}};

#if LUA_VERSION_NUM <= 502
static int f_clash(lua_State *L) {
  luaL_register(L, "clash", funcs);
  return 1;
}
#endif

static int counter = 0;
static int f_open_mod(lua_State *L) {
  counter++;
  lua_newtable(L);
  lua_pushstring(L, lua_tostring(L, 1));
  lua_setfield(L, -2, "name");
  return 1;
}

int main(void) {
  const struct lib *lib;
  lua_State *L;
  /* 5.1's package and io libraries set the environment of the running C
     function, so they open only through a call */
  for (lib = libs; lib->name; lib++)
    if (LUA_VERSION_NUM >= 502 || (lib->f != luaopen_package && lib->f != luaopen_io))
      open_direct(lib);
  for (lib = libs; lib->name; lib++) open_call(lib);
  /* luaL_openlibs */
  L = luaL_newstate();
  lua_pushstring(L, "below");
  luaL_openlibs(L);
  printf("openlibs: top=%d\n", lua_gettop(L));
  census(L, "openlibs");
  dochunk(L, "print('  package.loaded is _LOADED', package.loaded == debug.getregistry()._LOADED)\n"
             "print('  require string', require('string') == string)\n"
             "print('  preload', type(package.preload))\n"
             "print('  version', _VERSION)");
#if LUA_VERSION_NUM >= 504
  dochunk(L, "warn('first') warn('@on') warn('second') warn('a', 'b') warn('@off') warn('third')");
#endif
  luaL_openlibs(L);
  printf("openlibs twice: top=%d\n", lua_gettop(L));
  lua_close(L);
  L = luaL_newstate();
#if LUA_VERSION_NUM >= 502
  /* luaL_requiref and luaL_getsubtable */
  luaL_requiref(L, "mymod", f_open_mod, 1);
  show_from(L, "requiref", 1);
  census(L, "requiref");
  lua_getglobal(L, "mymod");
  printf("  global is result: %d\n", lua_rawequal(L, -1, 1));
  lua_settop(L, 0);
  luaL_requiref(L, "mymod", f_open_mod, 0);
  printf("requiref again: top=%d opened=%d\n", lua_gettop(L), counter);
  lua_settop(L, 0);
  luaL_requiref(L, "other", f_open_mod, 0);
  lua_getglobal(L, "other");
  printf("requiref no global: top=%d global=%s\n", lua_gettop(L), luaL_typename(L, -1));
  lua_settop(L, 0);
  lua_newtable(L);
  printf("getsubtable new=%d", luaL_getsubtable(L, 1, "sub"));
  {
    int r_ = luaL_getsubtable(L, 1, "sub");
    printf(" again=%d top=%d\n", r_, lua_gettop(L));
  }
  printf("  same=%d\n", lua_rawequal(L, 2, 3));
  lua_pushinteger(L, 5);
  lua_setfield(L, 1, "num");
  {
    int r_ = luaL_getsubtable(L, 1, "num");
    printf("getsubtable over number=%d top=%d\n", r_, lua_gettop(L));
  }
  lua_settop(L, 0);
  /* luaL_setfuncs with upvalues */
  lua_newtable(L);
  lua_pushstring(L, "up1");
  luaL_setfuncs(L, funcs, 1);
  printf("setfuncs: top=%d\n", lua_gettop(L));
  lua_setglobal(L, "lib");
  dochunk(L, "print('  setfuncs', lib.a()) print('  setfuncs', lib.b())");
  {
    static const luaL_Reg holes[] = {{"x", f_b}, {"hole", NULL}, {NULL, NULL}};
#if LUA_VERSION_NUM >= 504
    lua_newtable(L);
    luaL_setfuncs(L, holes, 0);
    lua_getfield(L, -1, "hole");
    show_from(L, "setfuncs hole", 2);
    lua_settop(L, 0);
#else
    (void)holes;
#endif
  }
  {
    luaL_newlib(L, funcs);
    lua_getfield(L, -1, "b");
    printf("newlib: top=%d b=%s\n", lua_gettop(L), luaL_typename(L, -1));
    lua_settop(L, 0);
  }
#endif
#if LUA_VERSION_NUM <= 502
  /* luaL_register / luaL_openlib */
  lua_pushstring(L, "up");
  luaL_openlib(L, "reg.sub", funcs, 1);
  printf("openlib dotted: top=%d\n", lua_gettop(L));
  lua_settop(L, 0);
  dochunk(L, "print('  reg.sub.a', reg.sub.a())\n"
             "print('  loaded', package == nil and 'no package' or 'package')");
  luaL_register(L, "reg.sub", funcs);
  printf("register again: top=%d\n", lua_gettop(L));
  lua_settop(L, 0);
  lua_pushinteger(L, 3);
  lua_setglobal(L, "clash");
  run(L, "register clash", f_clash, NULL);
  lua_newtable(L);
  luaL_register(L, NULL, funcs);
  lua_getfield(L, -1, "b");
  printf("register NULL: top=%d b=%s\n", lua_gettop(L), luaL_typename(L, -1));
  lua_settop(L, 0);
#endif
#if LUA_VERSION_NUM == 501
  lua_newtable(L);
  printf("findtable new=%s", luaL_findtable(L, 1, "x.y", 0) == NULL ? "NULL" : "name");
  printf(" top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
  lua_pushinteger(L, 1);
  lua_setfield(L, 1, "n");
  {
    const char *r_ = luaL_findtable(L, 1, "n.z", 0);
    printf("findtable clash=%s top=%d\n", r_, lua_gettop(L));
  }
  lua_settop(L, 0);
#endif
#if LUA_VERSION_NUM == 502
  luaL_pushmodule(L, "pm", 2);
  printf("pushmodule: top=%d type=%s\n", lua_gettop(L), luaL_typename(L, -1));
  lua_settop(L, 0);
#endif
#if LUA_VERSION_NUM >= 505
  lua_close(L);
  L = luaL_newstate();
  luaL_openselectedlibs(L, LUA_GLIBK | LUA_STRLIBK, LUA_MATHLIBK);
  census(L, "selected");
  dochunk(L, "print('  preload math', type(debug) == 'nil', type(package))");
  lua_getfield(L, LUA_REGISTRYINDEX, "_PRELOAD");
  lua_getfield(L, -1, "math");
  printf("  preload math: %s top=%d\n", luaL_typename(L, -1), lua_gettop(L));
  lua_settop(L, 0);
#endif
  printf("top at end=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
