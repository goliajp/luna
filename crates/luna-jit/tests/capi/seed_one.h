/* Included ahead of every host built against luna (not against PUC): its
   states hash strings with seed 1, the seed the recordings' PUC was built
   with, so a host that prints something of a table's order prints it the
   same way on every run. 5.5's lua_newstate takes its seed from the host. */
#ifndef SEED_ONE_H
#define SEED_ONE_H

#include "lua.h"
#include "lauxlib.h"

#undef luaL_newstate
#define luaL_newstate()	luna_newstate_seeded(LUA_VERSION_NUM, 1)

#if LUA_VERSION_NUM < 505
#undef lua_newstate
#define lua_newstate(f,ud)	luna_newstate_with_seeded(LUA_VERSION_NUM, (f), (ud), 1)
#endif

#endif
