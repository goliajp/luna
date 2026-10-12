/*
 * API functions that allocate and may so raise a memory error: pushing
 * strings and C functions, new tables, userdata and threads, converting a
 * number to a string.
 */
#include "auxlib.h"

WRAP_R(const char *, lua_pushlstring, (lua_State *L, const char *s, size_t len), (L, s, len))
WRAP_R(const char *, lua_pushstring, (lua_State *L, const char *s), (L, s))
WRAP_R(const char *, lua_pushexternalstring,
       (lua_State *L, const char *s, size_t len, lua_Alloc falloc, void *ud),
       (L, s, len, falloc, ud))
WRAP_V(lua_pushcclosure, (lua_State *L, lua_CFunction fn, int n), (L, fn, n))
WRAP_V(lua_pushcfunction, (lua_State *L, lua_CFunction fn), (L, fn))
WRAP_V(lua_createtable, (lua_State *L, int narr, int nrec), (L, narr, nrec))
WRAP_R(void *, lua_newuserdatauv, (lua_State *L, size_t size, int nuv), (L, size, nuv))
WRAP_R(void *, lua_newuserdata, (lua_State *L, size_t size), (L, size))
WRAP_R(lua_State *, lua_newthread, (lua_State *L), (L))
WRAP_R(const char *, lua_tolstring, (lua_State *L, int idx, size_t *len), (L, idx, len))
WRAP_R(const char *, lua_tostring, (lua_State *L, int idx), (L, idx))
