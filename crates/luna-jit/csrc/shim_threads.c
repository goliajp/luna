/*
 * Thread functions that may throw: closing the running thread from inside
 * it ends the resume that runs it.
 */
#include "shim.h"

WRAP_R(int, lua_closethread, (lua_State *L, lua_State *from), (L, from))
WRAP_R(int, lua_resetthread, (lua_State *L), (L))
