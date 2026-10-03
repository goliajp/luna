/*
 * Loading and dumping chunks, the collector's controls, warnings and
 * allocation. The host functions luna calls here (readers, writers,
 * warning functions) may raise an error themselves, so each runs under an
 * error boundary of its own and the Rust side that called it gets the
 * status back instead of being jumped over.
 */
#include <stdlib.h>
#include "shim.h"

typedef const char *(*lua_Reader)(lua_State *L, void *ud, size_t *sz);
typedef int (*lua_Writer)(lua_State *L, const void *p, size_t sz, void *ud);
typedef void (*lua_WarnFunction)(void *ud, const char *msg, int tocont);

/* reader(L, ud, size) under a fresh boundary; the status an error inside
   threw goes to *status */
LUNA_HIDDEN const char *luna_c_protect_reader(lua_State *L, lua_Reader r, void *ud,
                                              size_t *size, int *status) {
  struct luna_G *g = G(L);
  struct luna_jmp j;
  const char *p = NULL;
  j.status = LUA_OK;
  j.previous = g->errjmp;
  g->errjmp = &j;
  if (setjmp(j.b) == 0)
    p = r(L, ud, size);
  g->errjmp = j.previous;
  *status = j.status;
  return p;
}

/* writer(L, p, sz, ud) under a fresh boundary */
LUNA_HIDDEN int luna_c_protect_writer(lua_State *L, lua_Writer w, const void *p, size_t sz,
                                      void *ud, int *status) {
  struct luna_G *g = G(L);
  struct luna_jmp j;
  int r = 0;
  j.status = LUA_OK;
  j.previous = g->errjmp;
  g->errjmp = &j;
  if (setjmp(j.b) == 0)
    r = w(L, p, sz, ud);
  g->errjmp = j.previous;
  *status = j.status;
  return r;
}

/* a warning function under a fresh boundary: the status it threw */
LUNA_HIDDEN int luna_c_protect_warn(lua_State *L, lua_WarnFunction f, void *ud,
                                    const char *msg, int tocont) {
  struct luna_G *g = G(L);
  struct luna_jmp j;
  j.status = LUA_OK;
  j.previous = g->errjmp;
  g->errjmp = &j;
  if (setjmp(j.b) == 0)
    f(ud, msg, tocont);
  g->errjmp = j.previous;
  return j.status;
}

/* luaL_newstate's allocation function (PUC l_alloc) */
LUNA_HIDDEN void *luna_c_l_alloc(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  (void)osize;
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  return realloc(ptr, nsize);
}

WRAP_R(int, lua_dump, (lua_State *L, lua_Writer writer, void *data, int strip),
       (L, writer, data, strip))
WRAP_R(int, luna_dump_51, (lua_State *L, lua_Writer writer, void *data), (L, writer, data))
WRAP_R(int, luna_gc_51, (lua_State *L, int what, int data), (L, what, data))
WRAP_V(lua_warning, (lua_State *L, const char *msg, int tocont), (L, msg, tocont))

int luna_capi_gc(lua_State *L, int what, int64_t a, int64_t b, int64_t c);

/* 5.4/5.5 lua_gc: the arguments each option of the dialect takes */
LUNA_HIDDEN int luna_c_lua_gc(lua_State *L, int what, ...) {
  int64_t a[3] = {0, 0, 0};
  int r;
  va_list ap;
  va_start(ap, what);
  if (G(L)->version >= 505) {
    if (what == 5) {
      a[0] = (int64_t)va_arg(ap, size_t);
    } else if (what == 9) {
      a[0] = va_arg(ap, int);
      a[1] = va_arg(ap, int);
    }
  } else {
    int n = (what == 5 || what == 6 || what == 7) ? 1 : what == 10 ? 2 : what == 11 ? 3 : 0;
    for (int i = 0; i < n; i++)
      a[i] = va_arg(ap, int);
  }
  va_end(ap);
  r = luna_capi_gc(L, what, a[0], a[1], a[2]);
  luna_check(L);
  return r;
}
