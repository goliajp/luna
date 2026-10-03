/* lua_ident, the text PUC's library carries (5.1's lua.h does not declare
   it, but its library defines it) */
#include <stdio.h>
#include "lua.h"

#if LUA_VERSION_NUM == 501
extern const char lua_ident[];
#endif

int main(void) {
  printf("[%s]\n", lua_ident);
  return 0;
}
