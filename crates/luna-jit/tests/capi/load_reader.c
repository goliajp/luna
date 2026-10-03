/* lua_load with readers: pieces, the end of the input, how far a reader is
   read when the chunk has a syntax error, modes, chunk names, errors a
   reader raises, and the stack around the load. */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM == 501
#define LOAD(L, r, d, n, m) lua_load(L, r, d, n)
#else
#define LOAD(L, r, d, n, m) lua_load(L, r, d, n, m)
#endif

/* hands out the pieces one per call, then NULL; counts the calls */
typedef struct {
  const char **pieces;
  int next;
  int calls;
  int verbose;
} Pieces;

static const char *pieces_reader(lua_State *L, void *ud, size_t *size) {
  Pieces *p = (Pieces *)ud;
  const char *s = p->pieces[p->next];
  (void)L;
  p->calls++;
  if (s == NULL) {
    if (p->verbose) printf("    reader call %d: end\n", p->calls);
    *size = 0;
    return NULL;
  }
  p->next++;
  *size = strlen(s);
  if (p->verbose) printf("    reader call %d: \"%s\"\n", p->calls, s);
  return s;
}

static const char *status_name(int st) {
  switch (st) {
    case LUA_OK: return "OK";
    case LUA_ERRRUN: return "ERRRUN";
    case LUA_ERRSYNTAX: return "ERRSYNTAX";
    case LUA_ERRMEM: return "ERRMEM";
    default: return "other";
  }
}

/* load the pieces, report the status and what is on top, run it if it
   loaded */
static void try_load(lua_State *L, const char *title, const char **pieces,
                     const char *name, const char *mode, int verbose) {
  Pieces p;
  int top = lua_gettop(L), st;
  p.pieces = pieces;
  p.next = 0;
  p.calls = 0;
  p.verbose = verbose;
  printf("%s\n", title);
  st = LOAD(L, pieces_reader, &p, name, mode);
  printf("  status %s, %d reader calls, %d new values\n", status_name(st), p.calls,
         lua_gettop(L) - top);
  if (st == LUA_OK) {
    st = lua_pcall(L, 0, 1, 0);
    if (st == LUA_OK)
      printf("  ran: %s\n", lua_isnil(L, -1) ? "nil" : lua_tostring(L, -1));
    else
      printf("  run error: %s\n", lua_tostring(L, -1));
  } else {
    printf("  message: %s\n", lua_tostring(L, -1));
  }
  lua_settop(L, top);
}

static const char *raising_reader(lua_State *L, void *ud, size_t *size) {
  int *calls = (int *)ud;
  (*calls)++;
  if (*calls == 1) {
    *size = 7;
    return "return ";
  }
  lua_pushstring(L, "reader failed");
  lua_error(L);
  return NULL;
}

static const char *table_raising_reader(lua_State *L, void *ud, size_t *size) {
  (void)ud;
  (void)size;
  lua_newtable(L);
  lua_pushinteger(L, 7);
  lua_setfield(L, -2, "code");
  lua_error(L);
  return NULL;
}

static const char *pushing_reader(lua_State *L, void *ud, size_t *size) {
  int *calls = (int *)ud;
  (*calls)++;
  printf("    reader sees %d values\n", lua_gettop(L));
  lua_pushinteger(L, *calls);
  if (*calls == 1) {
    *size = 9;
    return "return 42";
  }
  return NULL;
}

static const char *sized_reader(lua_State *L, void *ud, size_t *size) {
  int *calls = (int *)ud;
  (void)L;
  (*calls)++;
  if (*calls == 1) {
    *size = 0;
    return "return 1"; /* a piece of size 0 ends the input */
  }
  *size = 8;
  return "return 2";
}

static int load_in_c(lua_State *L) {
  int calls = 0, st;
  st = LOAD(L, raising_reader, &calls, "=inner", NULL);
  printf("  inside a C function: status %s after %d calls: %s\n", status_name(st),
         calls, lua_tostring(L, -1));
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  lua_pushinteger(L, 5);
  lua_setglobal(L, "five");

  {
    const char *p[] = {"local a = ", "five * ", "2\n", "return a + 1", NULL};
    try_load(L, "pieces of a valid chunk", p, "=pieces", NULL, 1);
  }
  {
    const char *p[] = {"return 'one piece'", NULL};
    try_load(L, "one piece", p, "=one", NULL, 1);
  }
  {
    const char *p[] = {NULL};
    try_load(L, "no input at all", p, "=empty", NULL, 1);
  }
  {
    const char *p[] = {"x = = 1\n", "this piece is never read", "nor this", NULL};
    try_load(L, "syntax error in the first piece", p, "=early", NULL, 1);
  }
  {
    const char *p[] = {"local x = 1\n", "local y = 2\n", "x = = 3\n", "y = 4\n",
                       "return x", NULL};
    try_load(L, "syntax error in the third piece", p, "=third", NULL, 1);
  }
  {
    const char *p[] = {"return 1 +", " 2 +", " 3", NULL};
    try_load(L, "an expression across pieces", p, "=expr", NULL, 1);
  }
  {
    const char *p[] = {"local abc", "def = 1 return abcdef", NULL};
    try_load(L, "a name split between pieces", p, "=name", NULL, 1);
  }
  {
    const char *p[] = {"return 0x", "10", NULL};
    try_load(L, "a number split between pieces", p, "=num", NULL, 1);
  }
  {
    const char *p[] = {"return 'unfinished", " string", NULL};
    try_load(L, "an unfinished string", p, "=str", NULL, 1);
  }
  {
    const char *p[] = {"return 1 +", NULL};
    try_load(L, "the input ends inside an expression", p, "=eof", NULL, 1);
  }
  {
    const char *p[] = {"return five", NULL};
    try_load(L, "a null chunk name", p, NULL, NULL, 0);
  }
  {
    const char *p[] = {"error('boom')", NULL};
    try_load(L, "a null chunk name in a runtime error", p, NULL, NULL, 0);
    try_load(L, "chunk name =custom", p, "=custom", NULL, 0);
    try_load(L, "chunk name @file.lua", p, "@file.lua", NULL, 0);
    try_load(L, "chunk name as source", p, "error('boom')", NULL, 0);
  }
  {
    const char *p[] = {"x = = 1", NULL};
    try_load(L, "a syntax error with a null chunk name", p, NULL, NULL, 0);
  }

#if LUA_VERSION_NUM >= 502
  {
    const char *p[] = {"return 'text'", NULL};
    const char *empty[] = {NULL};
    try_load(L, "mode t, text chunk", p, "=m", "t", 1);
    try_load(L, "mode b, text chunk", p, "=m", "b", 1);
    try_load(L, "mode bt, text chunk", p, "=m", "bt", 0);
    try_load(L, "mode empty, text chunk", p, "=m", "", 0);
    try_load(L, "mode xyz, text chunk", p, "=m", "xyz", 0);
    try_load(L, "mode b, empty chunk", empty, "=m", "b", 1);
    try_load(L, "null mode, text chunk", p, "=m", NULL, 0);
  }
#endif

  {
    int calls = 0, st, top = lua_gettop(L);
    printf("a reader that raises\n");
    st = LOAD(L, raising_reader, &calls, "=raise", NULL);
    printf("  status %s after %d calls, %d new values: %s\n", status_name(st), calls,
           lua_gettop(L) - top, lua_tostring(L, -1));
    lua_settop(L, top);
  }
  {
    int st, top = lua_gettop(L);
    printf("a reader that raises a table\n");
    st = LOAD(L, table_raising_reader, NULL, "=raise", NULL);
    printf("  status %s, %d new values, a %s", status_name(st), lua_gettop(L) - top,
           luaL_typename(L, -1));
    lua_getfield(L, -1, "code");
    printf(" with code %d\n", (int)lua_tointeger(L, -1));
    lua_settop(L, top);
  }
  {
    int calls = 0, st, top;
    printf("a reader that pushes values\n");
    lua_pushstring(L, "below");
    top = lua_gettop(L);
    st = LOAD(L, pushing_reader, &calls, "=push", NULL);
    printf("  status %s, %d new values, top is a %s\n", status_name(st),
           lua_gettop(L) - top, luaL_typename(L, -1));
    lua_settop(L, 0);
  }
  {
    int calls = 0, st;
    printf("a piece of size 0\n");
    st = LOAD(L, sized_reader, &calls, "=sized", NULL);
    printf("  status %s after %d calls\n", status_name(st), calls);
    lua_call(L, 0, 1);
    printf("  ran: %s\n", lua_isnil(L, -1) ? "nil" : lua_tostring(L, -1));
    lua_settop(L, 0);
  }
  {
    int st;
    printf("a reader that raises inside a C function\n");
    lua_pushcfunction(L, load_in_c);
    st = lua_pcall(L, 0, 0, 0);
    printf("  pcall status %s\n", status_name(st));
  }
  {
    /* the loaded chunk sees the globals, also when the C function that
       loads it has other upvalues around */
    const char *p[] = {"five = five + 1 return five", NULL};
    try_load(L, "the chunk's globals", p, "=globals", NULL, 0);
  }
  lua_close(L);
  return 0;
}
