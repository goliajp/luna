-- v2.12 CORPUS-III: _ENV can be shadowed; free names resolve
-- through the innermost _ENV. Capture needed builtins as locals
-- first — inside the shadow, print/rawget themselves would
-- resolve via the new (empty-ish) _ENV.
local g_print = print
local g_rawget = rawget
do
  local _ENV = { name = "chunk_env" }
  g_print(name)
  x = 10
  g_print(x, g_rawget(_ENV, "x"))
end
print(name, x)
