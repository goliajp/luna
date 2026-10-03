/*
 * Error boundaries, throwing, and the default panic function.
 */
#include <stdio.h>
#include <stdlib.h>
#include "shim.h"

/* call f(L) under a fresh boundary: LUA_OK and its result count in *nret,
   or the status an error or yield inside threw */
LUNA_HIDDEN int luna_c_protect(lua_State *L, lua_CFunction f, int *nret) {
  struct luna_G *g = G(L);
  struct luna_jmp j;
  j.status = LUA_OK;
  j.previous = g->errjmp;
  g->errjmp = &j;
  if (setjmp(j.b) == 0)
    *nret = f(L);
  g->errjmp = j.previous;
  return j.status;
}

/* a 5.3+ continuation function under a fresh boundary */
LUNA_HIDDEN int luna_c_protect_k(lua_State *L, lua_KFunction k, int status,
                                 lua_KContext ctx, int *nret) {
  struct luna_G *g = G(L);
  struct luna_jmp j;
  j.status = LUA_OK;
  j.previous = g->errjmp;
  g->errjmp = &j;
  if (setjmp(j.b) == 0)
    *nret = k(L, status, ctx);
  g->errjmp = j.previous;
  return j.status;
}

/* a hook function under a fresh boundary */
LUNA_HIDDEN int luna_c_protect_hook(lua_State *L, lua_Hook h, lua_Debug *ar) {
  struct luna_G *g = G(L);
  struct luna_jmp j;
  j.status = LUA_OK;
  j.previous = g->errjmp;
  g->errjmp = &j;
  if (setjmp(j.b) == 0)
    h(L, ar);
  g->errjmp = j.previous;
  return j.status;
}

/* a host function that may itself raise (a lua_Alloc, a reader, a writer, a
   warning function) is called by the Rust side directly: those never run
   API functions that throw */

/* PUC luaD_throw: jump to the innermost boundary; with none, the error is
   unprotected: call the panic function and end the process (5.1 exits
   with EXIT_FAILURE, later versions abort) */
LUNA_HIDDEN void luna_throw(lua_State *L, int status) {
  struct luna_G *g = G(L);
  if (g->errjmp != NULL) {
    g->errjmp->status = status;
    longjmp(g->errjmp->b, 1);
  }
  if (g->panic != NULL)
    g->panic(g->err_from != NULL ? g->err_from : L);
  if (g->version == 501)
    exit(EXIT_FAILURE);
  abort();
}

/* luaL_newstate's panic function, per version */
LUNA_HIDDEN int luna_c_default_panic(lua_State *L) {
  const char *msg;
  if (G(L)->version >= 504) {
    size_t len;
    msg = luna_capi_tolstring(L, -1, &len);
    if (msg == NULL || luna_capi_type(L, -1) != 4)
      msg = "error object is not a string";
  } else {
    size_t len;
    msg = luna_capi_tolstring(L, -1, &len);
  }
  fprintf(stderr, "PANIC: unprotected error in call to Lua API (%s)\n", msg);
  fflush(stderr);
  return 0;
}

/* lua_error: the error object is on top of L */
LUNA_HIDDEN int luna_c_lua_error(lua_State *L) {
  luna_capi_error_prepare(L);
  luna_throw(L, LUA_ERRRUN);
}

/* luna's standard output, written through this C library's stdout so it
   interleaves with the host's own output as PUC's does */
LUNA_HIDDEN int luna_c_stdout_write(const void *p, size_t n) {
  return fwrite(p, 1, n, stdout) == n;
}

LUNA_HIDDEN int luna_c_stdout_flush(void) {
  return fflush(stdout) == 0;
}

LUNA_HIDDEN void luna_c_stdout_setvbuf(int mode) {
  static const int modes[3] = {_IOFBF, _IOLBF, _IONBF};
  setvbuf(stdout, NULL, modes[mode], BUFSIZ);
}
