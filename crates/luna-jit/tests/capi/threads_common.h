/* helpers shared by the threads_* and cont_* C API tests */
#ifndef THREADS_COMMON_H
#define THREADS_COMMON_H
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

/* lua_resume in each dialect's shape; nres is what 5.4+ reports, the
   thread's top before */
static int resume(lua_State *co, lua_State *from, int nargs, int *nres) {
#if LUA_VERSION_NUM >= 504
  return lua_resume(co, from, nargs, nres);
#elif LUA_VERSION_NUM >= 502
  int st = lua_resume(co, from, nargs);
  *nres = lua_gettop(co);
  return st;
#else
  int st = lua_resume(co, nargs);
  (void)from;
  *nres = lua_gettop(co);
  return st;
#endif
}

/* print the value at idx without converting it in place */
static void show_value(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  switch (t) {
    case LUA_TNUMBER:
    case LUA_TSTRING:
      lua_pushvalue(L, idx);
      printf("%s", lua_tostring(L, -1));
      lua_pop(L, 1);
      break;
    case LUA_TBOOLEAN:
      printf("%s", lua_toboolean(L, idx) ? "true" : "false");
      break;
    case LUA_TNIL:
      printf("nil");
      break;
    default:
      printf("<%s>", lua_typename(L, t));
  }
}

/* print every value of L's current frame */
static void show_stack(lua_State *L, const char *tag) {
  int i, n = lua_gettop(L);
  printf("%s: top=%d [", tag, n);
  for (i = 1; i <= n; i++) {
    if (i > 1) printf(" ");
    show_value(L, i);
  }
  printf("]\n");
}

/* resume co with the top nargs values of co and print what came back */
static int resume_show(lua_State *co, lua_State *from, int nargs, const char *tag) {
  int nres = -1;
  int st = resume(co, from, nargs, &nres);
  printf("%s: status=%d nres=%d lua_status=%d ", tag, st, nres, lua_status(co));
  show_stack(co, "co");
  return st;
}

/* push the value a chunk returns */
static void eval(lua_State *L, const char *src) {
  if (luaL_loadstring(L, src) != 0 || lua_pcall(L, 0, 1, 0) != 0) {
    printf("eval failed: %s\n", lua_tostring(L, -1));
  }
}

/* run a chunk, printing its error if any */
static void run(lua_State *L, const char *src) {
  if (luaL_loadstring(L, src) != 0 || lua_pcall(L, 0, 0, 0) != 0) {
    printf("run failed: %s\n", lua_tostring(L, -1));
    lua_pop(L, 1);
  }
}

static void reg(lua_State *L, const char *name, lua_CFunction f) {
  lua_pushcfunction(L, f);
  lua_setglobal(L, name);
}

/* continuation functions: 5.2's are lua_CFunctions that ask lua_getctx */
#if LUA_VERSION_NUM == 502
typedef int kctx_t;
#define KDEF(name) static int name(lua_State *L)
#define KARGS int ctx_; int status = lua_getctx(L, &ctx_); kctx_t ctx = ctx_
#elif LUA_VERSION_NUM >= 503
typedef lua_KContext kctx_t;
#define KDEF(name) static int name(lua_State *L, int status, lua_KContext ctx)
#define KARGS (void)0
#endif

#endif
