/* lua_ident, the text PUC's library carries (5.1's lua.h does not declare
   it, but its library defines it) */
#include <stdio.h>
#include "lua.h"

/* luna's 5.1 lua.h declares it (with the DLL import Windows needs) */
#if LUA_VERSION_NUM == 501 && !defined(LUNA_DATA)
extern const char lua_ident[];
#endif

int main(void) {
  printf("[%s]\n", lua_ident);
  return 0;
}
