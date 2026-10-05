/* helpers shared by the alloc_* C API tests: an allocation function that
   counts what it hands out and refuses as told, and the outcome of a run
   in a form that does not depend on how many allocations an
   implementation makes */
#ifndef ALLOC_COMMON_H
#define ALLOC_COMMON_H
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM >= 505
#define NEWSTATE(f, ud) lua_newstate(f, ud, 0)
#else
#define NEWSTATE(f, ud) lua_newstate(f, ud)
#endif

/* how the allocation function refuses */
enum { NEVER, FROM_NTH, ONLY_NTH, LIMIT };

typedef struct {
  int mode;
  long n;       /* allocations still to grant before refusing */
  size_t limit; /* LIMIT: the most bytes it hands out at once */
  size_t live;
  long blocks;
  int refused; /* it refused something since this was last cleared */
} Ctl;

/* a growth (a new block or a bigger one) may be refused; shrinking and
   freeing never fail */
static void *ctl_alloc(void *ud, void *ptr, size_t osize, size_t nsize) {
  Ctl *c = (Ctl *)ud;
  size_t old = ptr != NULL ? osize : 0;
  void *q;
  if (nsize == 0) {
    if (ptr != NULL) {
      c->live -= osize;
      c->blocks--;
    }
    free(ptr);
    return NULL;
  }
  if (nsize > old) {
    switch (c->mode) {
      case FROM_NTH:
        if (c->n == 0) {
          c->refused = 1;
          return NULL;
        }
        c->n--;
        break;
      case ONLY_NTH:
        if (c->n-- == 0) {
          c->refused = 1;
          return NULL;
        }
        break;
      case LIMIT:
        if (c->live - old + nsize > c->limit) {
          c->refused = 1;
          return NULL;
        }
        break;
    }
  }
  q = realloc(ptr, nsize);
  if (q == NULL) return NULL;
  if (ptr == NULL) c->blocks++;
  c->live = c->live - old + nsize;
  return q;
}

static const char *status_name(int st) {
  switch (st) {
    case 0: return "ok";
    case LUA_ERRRUN: return "LUA_ERRRUN";
    case LUA_ERRSYNTAX: return "LUA_ERRSYNTAX";
    case LUA_ERRMEM: return "LUA_ERRMEM";
    default: return "other";
  }
}

/* the distinct outcomes of a sweep, kept sorted */
#define MAXOUT 16
typedef struct {
  char s[MAXOUT][96];
  int n;
} Outcomes;

static void add_outcome(Outcomes *o, const char *s) {
  int i, j;
  for (i = 0; i < o->n; i++) {
    int c = strcmp(o->s[i], s);
    if (c == 0) return;
    if (c > 0) break;
  }
  if (o->n == MAXOUT) return;
  for (j = o->n; j > i; j--) strcpy(o->s[j], o->s[j - 1]);
  strncpy(o->s[i], s, sizeof(o->s[i]) - 1);
  o->s[i][sizeof(o->s[i]) - 1] = '\0';
  o->n++;
}

static void print_outcomes(const char *what, Outcomes *o) {
  int i;
  printf("%s:", what);
  for (i = 0; i < o->n; i++) printf(" [%s]", o->s[i]);
  printf("\n");
}

/* the outcome of a protected call that left status st and its result or
   error on top */
static void outcome_of(lua_State *L, int st, char *buf, size_t size) {
  const char *msg = lua_type(L, -1) == LUA_TSTRING ? lua_tostring(L, -1)
                    : lua_type(L, -1) == LUA_TNUMBER ? lua_tostring(L, -1)
                    : lua_type(L, -1) == LUA_TNONE ? "" : lua_typename(L, lua_type(L, -1));
  snprintf(buf, size, "%s %s", status_name(st), msg);
}

/* luaL_openlibs under protection */
static int openlibs(lua_State *L) {
  luaL_openlibs(L);
  return 0;
}

/* the calls of lua_pcall's shape for every dialect */
static int pcall(lua_State *L, int nargs, int nres) {
  return lua_pcall(L, nargs, nres, 0);
}
#endif
