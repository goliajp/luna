-- v2.14 HD 5.1 seed: `goto` is NOT a keyword in 5.1 — usable as
-- an identifier; a 5.2-style goto statement fails to compile.
local goto_var = loadstring("local goto = 5; return goto")
print(goto_var ~= nil and goto_var())
local f = loadstring("goto done; ::done::")
print(f == nil)
