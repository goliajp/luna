/*
** luna's luaconf.h for the Lua 5.1 dialect: the configuration of a
** default 64-bit build, as PUC-Rio's luaconf.h gives it.
** See Copyright Notice in lua.h
*/


#ifndef lconfig_h
#define lconfig_h

#include <limits.h>
#include <stddef.h>


/* names of the environment variables Lua checks for paths and for
   initialization code */
#define LUA_PATH        "LUA_PATH"
#define LUA_CPATH       "LUA_CPATH"
#define LUA_INIT	"LUA_INIT"


/* no install prefix: only the current-directory templates */
#define LUA_PATH_DEFAULT	"./?.lua;./?/init.lua"
#define LUA_CPATH_DEFAULT	""

#if defined(_WIN32)
#define LUA_DIRSEP	"\\"
#else
#define LUA_DIRSEP	"/"
#endif

#define LUA_PATHSEP	";"
#define LUA_PATH_MARK	"?"
#define LUA_EXECDIR	"!"
#define LUA_IGMARK	"-"


/* the integral type used by lua_pushinteger/lua_tointeger */
#define LUA_INTEGER	ptrdiff_t


/*
** Marks for exported symbols
*/
#if defined(LUA_BUILD_AS_DLL)

#if defined(LUA_CORE) || defined(LUA_LIB)
#define LUA_API __declspec(dllexport)
#else
#define LUA_API __declspec(dllimport)
#endif

#else

#define LUA_API		extern

#endif

#define LUALIB_API	LUA_API


/* quote names in messages */
#define LUA_QL(x)	"'" x "'"
#define LUA_QS		LUA_QL("%s")


/* maximum size for the description of the source of a function */
#define LUA_IDSIZE	60


/*
** Compatibility with Lua 5.0, as PUC's default build has it
*/
#undef LUA_COMPAT_GETN
#undef LUA_COMPAT_LOADLIB
#define LUA_COMPAT_VARARG
#define LUA_COMPAT_MOD
#define LUA_COMPAT_LSTR		1
#define LUA_COMPAT_GFIND
#define LUA_COMPAT_OPENLIB


/* initial buffer size of the lauxlib buffer system (it needs <stdio.h>) */
#define LUAL_BUFFERSIZE		BUFSIZ


/*
** Number type: 'double' (fixed: it is part of the ABI)
*/
#define LUA_NUMBER_DOUBLE
#define LUA_NUMBER	double

#define LUAI_UACNUMBER	double

#define LUA_NUMBER_SCAN		"%lf"
#define LUA_NUMBER_FMT		"%.14g"


/* length modifier and type of integer formats in 'string.format' */
#define LUA_INTFRMLEN		"l"
#define LUA_INTFRM_T		long


#endif
