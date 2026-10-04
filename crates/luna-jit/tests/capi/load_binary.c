/* lua_dump and loading binary chunks back: round trips, stripping,
   writers that fail or raise, values that cannot be dumped, binary chunks
   in pieces, truncated chunks, trailing bytes and modes. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#ifndef LUA_OK
#define LUA_OK 0
#endif

#if LUA_VERSION_NUM == 501
#define LOAD(L, r, d, n, m) lua_load(L, r, d, n)
#else
#define LOAD(L, r, d, n, m) lua_load(L, r, d, n, m)
#endif
#if LUA_VERSION_NUM >= 503
#define DUMP(L, w, d, s) lua_dump(L, w, d, s)
#else
#define DUMP(L, w, d, s) lua_dump(L, w, d)
#endif

#define ERRSRC "local x = ...\nerror('boom ' .. x)"

typedef struct {
  char *b;
  size_t n;
  int last_was_end; /* the last call had no bytes and a null pointer */
} Buf;

static int buf_writer(lua_State *L, const void *p, size_t sz, void *ud) {
  Buf *b = (Buf *)ud;
  (void)L;
  b->last_was_end = (p == NULL && sz == 0);
  if (sz > 0) {
    b->b = (char *)realloc(b->b, b->n + sz);
    memcpy(b->b + b->n, p, sz);
    b->n += sz;
  }
  return 0;
}

/* hands out a block in pieces of `step` bytes */
typedef struct {
  const char *b;
  size_t n, at, step;
  int calls;
} Feed;

static const char *feed_reader(lua_State *L, void *ud, size_t *size) {
  Feed *f = (Feed *)ud;
  size_t k;
  (void)L;
  f->calls++;
  if (f->at >= f->n) {
    *size = 0;
    return NULL;
  }
  k = f->n - f->at < f->step ? f->n - f->at : f->step;
  *size = k;
  f->at += k;
  return f->b + f->at - k;
}

static int load_block(lua_State *L, const char *b, size_t n, size_t step, const char *name,
                      const char *mode, int *calls) {
  Feed f;
  int st;
  f.b = b;
  f.n = n;
  f.at = 0;
  f.step = step;
  f.calls = 0;
  st = LOAD(L, feed_reader, &f, name, mode);
  *calls = f.calls;
  return st;
}

static void dump_top(lua_State *L, Buf *b, int strip) {
  int st;
  b->b = NULL;
  b->n = 0;
  b->last_was_end = 0;
  st = DUMP(L, buf_writer, b, strip);
  if (st != 0 || b->n < 5) {
    printf("  dump status %d, %d bytes\n", st, (int)b->n);
    return;
  }
  printf("  dump status %d, header %02x %c%c%c version %02x\n", st, (unsigned char)b->b[0],
         b->b[1], b->b[2], b->b[3], (unsigned char)b->b[4]);
#if LUA_VERSION_NUM >= 505
  printf("  the last writer call marks the end: %s\n", b->last_was_end ? "yes" : "no");
#endif
}

static void call_report(lua_State *L, int nargs) {
  int st = lua_pcall(L, nargs, 1, 0);
  if (st == LUA_OK)
    printf("  call: %s\n", lua_tostring(L, -1));
  else
    printf("  call error %d: %s\n", st, lua_tostring(L, -1));
  lua_pop(L, 1);
}

static int fail_writer(lua_State *L, const void *p, size_t sz, void *ud) {
  int *calls = (int *)ud;
  (void)L;
  (void)p;
  (void)sz;
  (*calls)++;
  return 42;
}

static int raising_writer(lua_State *L, const void *p, size_t sz, void *ud) {
  (void)p;
  (void)sz;
  (void)ud;
  lua_pushstring(L, "writer failed");
  lua_error(L);
  return 0;
}

static int dump_raising(lua_State *L) {
  int st;
  luaL_loadstring(L, "return 1");
  st = DUMP(L, raising_writer, NULL, 0);
  printf("  NOT REACHED: lua_dump returned %d\n", st);
  return 0;
}

static int a_c_function(lua_State *L) {
  (void)L;
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  Buf b, s;
  int st, calls, top;
  luaL_openlibs(L);

  printf("round trip of a chunk\n");
  luaL_loadstring(L, "local a, b = ... return 'sum ' .. (a + b)");
  dump_top(L, &b, 0);
  printf("  the function is still on the stack: %s\n", lua_isfunction(L, -1) ? "yes" : "no");
  lua_pop(L, 1);
  st = load_block(L, b.b, b.n, b.n, "=bin", NULL, &calls);
  printf("  load status %d after %d reader calls\n", st, calls);
  lua_pushinteger(L, 3);
  lua_pushinteger(L, 4);
  call_report(L, 2);

  printf("pieces of 7 bytes\n");
  st = load_block(L, b.b, b.n, 7, "=bin", NULL, &calls);
  printf("  load status %d after %d reader calls (%d pieces)\n", st, calls,
         (int)((b.n + 6) / 7));
  lua_pushinteger(L, 10);
  lua_pushinteger(L, 20);
  call_report(L, 2);

  printf("pieces of 1 byte\n");
  st = load_block(L, b.b, b.n, 1, "=bin", NULL, &calls);
  printf("  load status %d after %d reader calls for %d bytes\n", st, calls, (int)b.n);
  lua_pop(L, 1);

  printf("trailing bytes after the chunk\n");
  {
    char *t = (char *)malloc(b.n + 5);
    memcpy(t, b.b, b.n);
    memcpy(t + b.n, "xxxxx", 5);
    st = load_block(L, t, b.n + 5, b.n + 5, "=bin", NULL, &calls);
    printf("  load status %d after %d reader calls\n", st, calls);
    if (st == LUA_OK) {
      lua_pushinteger(L, 1);
      lua_pushinteger(L, 2);
      call_report(L, 2);
    } else {
      printf("  message: %s\n", lua_tostring(L, -1));
      lua_pop(L, 1);
    }
    free(t);
  }

  printf("a truncated chunk\n");
  st = load_block(L, b.b, b.n - 3, b.n, "=trunc", NULL, &calls);
  printf("  load status %d after %d reader calls: %s\n", st, calls, lua_tostring(L, -1));
  lua_pop(L, 1);
  st = load_block(L, b.b, 3, b.n, "@trunc.luac", NULL, &calls);
  printf("  only 3 bytes: status %d: %s\n", st, lua_tostring(L, -1));
  lua_pop(L, 1);

#if LUA_VERSION_NUM >= 502
  printf("modes\n");
  st = load_block(L, b.b, b.n, b.n, "=bin", "t", &calls);
  printf("  mode t: status %d after %d calls: %s\n", st, calls, lua_tostring(L, -1));
  lua_pop(L, 1);
  st = load_block(L, b.b, b.n, b.n, "=bin", "b", &calls);
  printf("  mode b: status %d\n", st);
  lua_pop(L, 1);
#endif

  printf("a function with upvalues\n");
  luaL_loadstring(L, "local n = 10 return function(x) n = n + x return n end");
  lua_call(L, 0, 1);
  dump_top(L, &s, 0);
  lua_pop(L, 1);
  st = load_block(L, s.b, s.n, s.n, "=up", NULL, &calls);
  printf("  load status %d\n", st);
  lua_pushinteger(L, 1);
  call_report(L, 1);
  free(s.b);

  printf("a chunk using globals\n");
  luaL_loadstring(L, "g = (g or 0) + 1 return 'g=' .. g");
  dump_top(L, &s, 0);
  lua_pop(L, 1);
  load_block(L, s.b, s.n, s.n, "=glob", NULL, &calls);
  call_report(L, 0);
  load_block(L, s.b, s.n, s.n, "=glob", NULL, &calls);
  call_report(L, 0);
  free(s.b);

  printf("errors in a dumped function\n");
  load_block(L, ERRSRC, strlen(ERRSRC), strlen(ERRSRC), "=src", NULL, &calls);
  dump_top(L, &s, 0);
  lua_pop(L, 1);
  load_block(L, s.b, s.n, s.n, "=other name", NULL, &calls);
  lua_pushstring(L, "plain");
  call_report(L, 1);
  free(s.b);
#if LUA_VERSION_NUM >= 503
  printf("stripped\n");
  load_block(L, ERRSRC, strlen(ERRSRC), strlen(ERRSRC), "=src", NULL, &calls);
  dump_top(L, &s, 1);
  lua_pop(L, 1);
  load_block(L, s.b, s.n, s.n, "=other name", NULL, &calls);
  lua_pushstring(L, "stripped");
  call_report(L, 1);
  free(s.b);
#endif

  printf("a writer that fails\n");
  luaL_loadstring(L, "return 1");
  calls = 0;
  st = DUMP(L, fail_writer, &calls, 0);
  printf("  status %d after %d writer calls\n", st, calls);
  lua_pop(L, 1);

  printf("a writer that raises\n");
  lua_pushcfunction(L, dump_raising);
  st = lua_pcall(L, 0, 0, 0);
  printf("  pcall status %d: %s\n", st, lua_tostring(L, -1));
  lua_pop(L, 1);

#if LUA_VERSION_NUM <= 504
  printf("values that are not Lua functions\n");
  top = lua_gettop(L);
  lua_pushcfunction(L, a_c_function);
  calls = 0;
  st = DUMP(L, fail_writer, &calls, 0);
  printf("  C function: status %d, %d writer calls\n", st, calls);
  lua_pushinteger(L, 5);
  st = DUMP(L, fail_writer, &calls, 0);
  printf("  number: status %d, %d writer calls, %d values\n", st, calls, lua_gettop(L) - top);
  lua_settop(L, top);
#else
  (void)a_c_function;
  (void)top;
#endif
  free(b.b);
  lua_close(L);
  return 0;
}
