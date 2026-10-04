/* the io library: opening, writing and reading files in every format,
   lines, seek, setvbuf, default files, closing, the errors of each, and
   what io.type and tostring say. Files whose offsets are printed are
   binary, which Windows does not translate */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static const char *script =
  "local name = 'luna_io_files.tmp'\n"
  "local function show(...)\n"
  "  local t = table.pack and table.pack(...) or {n = select('#', ...), ...}\n"
  "  local out = {}\n"
  "  for i = 1, t.n do\n"
  "    local v = t[i]\n"
  "    if io.type(v) then v = io.type(v) .. ':' .. tostring(v):gsub('0x%x+', 'P'):gsub('%(%x+%)', '(P)') end\n"
  "    out[i] = type(v) == 'string' and ('%q'):format(v) or tostring(v)\n"
  "  end\n"
  "  print(table.concat(out, ' '))\n"
  "end\n"
  "local f = assert(io.open(name, 'wb'))\n"
  "show(io.type(f), io.type(io.stdout), io.type(42))\n"
  "show(f:write('line one\\n', 'line two\\n', 12, ' ', 3.5, ' ', -0.25, '\\n'))\n"
  "f:write('0x1F 1e3 .5 -7 nan\\n', 'last')\n"
  "show(f:seek('cur'), f:seek('set', 2), f:seek('end'))\n"
  "show(f:setvbuf('full', 1024), f:setvbuf('no'), f:setvbuf('line'))\n"
  "show(f:close())\n"
  "show(io.type(f), tostring(f))\n"
  "show(pcall(f.write, f, 'x'))\n"
  "show(pcall(f.close, f))\n"
  "f = assert(io.open(name, 'rb'))\n"
  "show(f:read('*l'))\n"
  "show(f:read(_VERSION == 'Lua 5.1' and '*l' or '*L'))\n"
  "if _VERSION == 'Lua 5.1' then\n"
  "  show(f:read('*n', '*n', '*n'))\n"
  "  show(f:read('*l'))\n"
  "  show(f:read('*n', '*n', '*n', '*n', '*n'))\n"
  "else\n"
  "  show(f:read('*n', '*n', '*n'))\n"
  "  show(f:read('*l'))\n"
  "  show(f:read('*n', '*n', '*n', '*n', '*n'))\n"
  "end\n"
  "show(f:read(0), f:read(3), f:read('*a'), f:read('*a'), f:read(0), f:read(1), f:read('*l'))\n"
  "show(pcall(f.read, f, '*x'))\n"
  "show(pcall(f.read, f, 'x'))\n"
  "show(f:seek('set', 0), f:read(4))\n"
  "f:close()\n"
  "for l in io.lines(name) do io.write('[', l, ']') end print()\n"
  "f = io.open(name)\n"
  "for l in f:lines() do io.write('<', #l, '>') end print()\n"
  "show(io.type(f), f:read('*a'))\n"
  "f:close()\n"
  "if _VERSION ~= 'Lua 5.1' then\n"
  "  for a, b in io.lines(name, 4, '*l') do show(a, b) break end\n"
  "  show(select('#', io.lines(name)))\n"
  "end\n"
  "show(io.input() == io.stdin, io.output() == io.stdout)\n"
  "io.input(name)\n"
  "show(io.read(), io.read('*n'))\n"
  "io.input():close()\n"
  "show(pcall(io.read))\n"
  "io.input(io.stdin)\n"
  "local g = io.output(name .. '2')\n"
  "show(io.write('to default ', 1, '\\n'), io.output() == g)\n"
  "show(io.close())\n"
  "show(pcall(io.write, 'x'))\n"
  "io.output(io.stdout)\n"
  "show(io.open(name .. '2'):read('*a'))\n"
  "show(io.open('no/such/dir/file'))\n"
  "show(pcall(io.lines, 'no/such/file'))\n"
  "show(pcall(io.open, name, 'rw'))\n"
  "show(pcall(io.input, 'no/such/file'))\n"
  "show(io.stdout:close())\n"
  "show(io.type(io.stdout))\n"
  "local t = io.tmpfile()\n"
  "t:write('temp') t:seek('set') show(t:read('*a')) t:close()\n"
  "do local h = io.open(name, 'a+b') h:write('\\nappended') h:seek('set') show(#h:read('*a')) end\n"
  "collectgarbage() collectgarbage()\n"
  "show(os.remove(name), os.remove(name .. '2'))\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  st = luaL_loadstring(L, script);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  if (st) printf("error: %s\n", lua_tostring(L, -1));
  fflush(stdout);
  lua_close(L);
  return 0;
}
