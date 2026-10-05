/* an allocation function that refuses every growth from the Nth on, swept
   over N until each scenario succeeds: the outcomes a scenario can end
   with, and that lua_close gives every block back each time. */
#include "alloc_common.h"

static const char *scripts[] = {
    /* strings, tables, closures, coroutines */
    "local t = {}\n"
    "for i = 1, 40 do t[i] = {k = 'v' .. i, f = function() return i end} end\n"
    "local co = coroutine.wrap(function(a) local b = coroutine.yield(a .. '!') return b .. '?' end)\n"
    "local x = co('a') .. co('b')\n"
    "return #t .. ' ' .. x .. ' ' .. t[40].f()\n",
    /* a pcall in Lua catches the memory error */
    "local ok, e = pcall(function() local t = {} for i = 1, 200 do t[i] = 'k' .. i end return #t end)\n"
    "return tostring(ok) .. ' ' .. tostring(e)\n",
    /* coroutine.resume reports it */
    "local co = coroutine.create(function() local t = {} for i = 1, 200 do t[i] = {i} end return #t end)\n"
    "local ok, e = coroutine.resume(co)\n"
    "return tostring(ok) .. ' ' .. tostring(e)\n",
    "return #string.rep('ab', 3000)\n",
    "local t = {} for i = 1, 300 do t[i] = i end return #table.concat(t, ',')\n",
    "return string.format('%s|%d|%5.1f|%q', string.rep('x', 300), 42, 1.5, 'q')\n",
};

static const char *names[] = {"script", "pcall in Lua", "coroutine.resume", "string.rep",
                              "table.concat", "string.format"};

/* the libraries without io: a 5.1 io library that runs out of memory half
   way through its opening can leave the C stdout closed by lua_close */
static const luaL_Reg some_libs[] = {
    {"_G", luaopen_base},       {LUA_TABLIBNAME, luaopen_table},
    {LUA_STRLIBNAME, luaopen_string}, {LUA_MATHLIBNAME, luaopen_math},
    {NULL, NULL}};

static int open_some(lua_State *L) {
  const luaL_Reg *lib;
  for (lib = some_libs; lib->func != NULL; lib++) {
#if LUA_VERSION_NUM == 501
    lua_pushcfunction(L, lib->func);
    lua_pushstring(L, lib->name);
    lua_call(L, 1, 0);
#else
    luaL_requiref(L, lib->name, lib->func, 1);
    lua_pop(L, 1);
#endif
  }
  return 0;
}

#define MAXN 200000

static int all_freed = 1;

/* a state with the standard libraries, then the budget */
static lua_State *ready(Ctl *c, long n) {
  lua_State *L;
  c->mode = NEVER;
  L = NEWSTATE(ctl_alloc, c);
  luaL_openlibs(L);
  c->mode = FROM_NTH;
  c->n = n;
  c->refused = 0;
  return L;
}

static void close_state(lua_State *L, Ctl *c) {
  lua_close(L);
  if (c->blocks != 0 || c->live != 0) all_freed = 0;
}

int main(void) {
  Outcomes o;
  char buf[96];
  long n;
  int i, done;
  Ctl c;

  memset(&o, 0, sizeof(o));
  for (n = 0, done = 0; n < MAXN && !done; n++) {
    lua_State *L;
    memset(&c, 0, sizeof(c));
    c.mode = FROM_NTH;
    c.n = n;
    L = NEWSTATE(ctl_alloc, &c);
    add_outcome(&o, L != NULL ? "state" : "NULL");
    done = !c.refused;
    if (L != NULL)
      close_state(L, &c);
    else if (c.blocks != 0)
      all_freed = 0;
  }
  print_outcomes("lua_newstate", &o);

  memset(&o, 0, sizeof(o));
  for (n = 0, done = 0; n < MAXN && !done; n++) {
    lua_State *L;
    int st;
    memset(&c, 0, sizeof(c));
    L = NEWSTATE(ctl_alloc, &c);
#if LUA_VERSION_NUM == 501
    c.mode = FROM_NTH;
    c.n = n;
    st = lua_cpcall(L, open_some, NULL);
#else
    lua_pushcfunction(L, open_some);
    c.mode = FROM_NTH;
    c.n = n;
    st = pcall(L, 0, 0);
#endif
    c.mode = NEVER; /* turning a number result into text allocates */
    outcome_of(L, st, buf, sizeof(buf));
    add_outcome(&o, buf);
    done = !c.refused;
    close_state(L, &c);
  }
  print_outcomes("opening the libraries", &o);

  memset(&o, 0, sizeof(o));
  for (n = 0, done = 0; n < MAXN && !done; n++) {
    lua_State *L = ready(&c, n);
    int st = luaL_loadstring(L, scripts[0]);
    c.mode = NEVER; /* turning a number result into text allocates */
    outcome_of(L, st, buf, sizeof(buf));
    add_outcome(&o, st == 0 ? "ok" : buf);
    done = !c.refused;
    close_state(L, &c);
  }
  print_outcomes("luaL_loadstring", &o);

  for (i = 0; i < (int)(sizeof(scripts) / sizeof(scripts[0])); i++) {
    memset(&o, 0, sizeof(o));
    for (n = 0, done = 0; n < MAXN && !done; n++) {
      lua_State *L = ready(&c, -1);
      int st;
      c.mode = NEVER;
      if (luaL_loadstring(L, scripts[i]) != 0) {
        printf("%s: does not load: %s\n", names[i], lua_tostring(L, -1));
        return 1;
      }
      c.mode = FROM_NTH;
      c.n = n;
      c.refused = 0;
      st = pcall(L, 0, 1);
      c.mode = NEVER; /* turning a number result into text allocates */
      outcome_of(L, st, buf, sizeof(buf));
      add_outcome(&o, buf);
      done = !c.refused;
      close_state(L, &c);
    }
    print_outcomes(names[i], &o);
  }
  printf("every block freed after lua_close: %s\n", all_freed ? "yes" : "no");
  return 0;
}
