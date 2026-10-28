-- a chain of `..` operands that runs out of registers: the error shows the
-- token the scanner is on and its line; 5.3 and 5.4 raise it on the first
-- token of the operand after the operator (a parenthesis, a unary operator,
-- a table, a function)
local load = loadstring or load
local locals = {}
for i = 1, 190 do locals[i] = "l" .. i end
local head = "local a, s = 1, 's'\nlocal " .. table.concat(locals, ", ") .. "\nreturn "
local firsts = {
  "(a)", "-a", "#s", "not a", "{}", "function() end", "(\n(a))", "-(\na)",
}
for _, first in ipairs(firsts) do
  local parts = {}
  for i = 1, 80 do parts[i] = (i % 2 == 0) and first or "a" end
  local src = head .. table.concat(parts, "\n.. ")
  local f, e = load(src, "=chain")
  print((first:gsub("\n", " ")), f and "loaded" or e)
end
