/*
 * The C side of luna's C API: the error boundary every call into a C
 * function runs under, and the API functions that may raise an error or
 * yield. An error leaves the C function at once, as in PUC Lua: the
 * function that raises it longjmps to the boundary set up just before the
 * C function was called. The boundary and every function a longjmp skips
 * are C; the Rust side has always returned before the jump.
 */
#ifndef LUNA_SHIM_H
#define LUNA_SHIM_H

#include <setjmp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>

#if defined(__GNUC__) || defined(__clang__)
#define LUNA_HIDDEN __attribute__((visibility("hidden")))
#define LUNA_NORETURN __attribute__((noreturn))
#elif defined(_MSC_VER)
#define LUNA_HIDDEN
#define LUNA_NORETURN __declspec(noreturn)
#else
#define LUNA_HIDDEN
#define LUNA_NORETURN
#endif

typedef struct luna_L lua_State;
typedef int (*lua_CFunction)(lua_State *L);
typedef intptr_t lua_KContext;
typedef int (*lua_KFunction)(lua_State *L, int status, lua_KContext ctx);
typedef struct lua_Debug lua_Debug;
typedef void (*lua_Hook)(lua_State *L, lua_Debug *ar);
typedef int64_t lua_Integer;
typedef double lua_Number;

#define LUA_OK 0
#define LUA_YIELD 1
#define LUA_ERRRUN 2

/* one error boundary: PUC's struct lua_longjmp */
struct luna_jmp {
  struct luna_jmp *previous;
  jmp_buf b;
  volatile int status;
};

/* the head of the Rust `Global` (capi/state.rs), shared by every thread */
struct luna_G {
  struct luna_jmp *errjmp;  /* innermost boundary, NULL outside any */
  int raised;               /* status a Rust API function asks to throw */
  int version;              /* LUA_VERSION_NUM of the state's dialect */
  lua_CFunction panic;      /* lua_atpanic's function */
  lua_State *err_from;      /* the thread whose stack top is the error */
};

/* the head of the Rust `LuaState` */
struct luna_L {
  struct luna_G *g;
};

#define G(L) ((L)->g)

LUNA_HIDDEN LUNA_NORETURN void luna_throw(lua_State *L, int status);

/* throw what a Rust API function raised, if it raised anything */
static inline void luna_check(lua_State *L) {
  struct luna_G *g = G(L);
  if (g->raised != 0) {
    int st = g->raised;
    g->raised = 0;
    luna_throw(L, st);
  }
}

/* functions of the Rust side the C side calls */
void luna_capi_raise_msg(lua_State *L, const char *msg);
const char *luna_capi_pushlstring(lua_State *L, const char *s, size_t len);
size_t luna_capi_num2str(lua_State *L, int isint, lua_Integer i, lua_Number n, char *buff);
void luna_capi_concat(lua_State *L, int n);
const char *luna_capi_tolstring(lua_State *L, int idx, size_t *len);
int luna_capi_type(lua_State *L, int idx);
void luna_capi_error_prepare(lua_State *L);
void luna_capi_where(lua_State *L, int level);

#endif
