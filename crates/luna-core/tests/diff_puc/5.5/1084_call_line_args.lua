-- the line a call is made on, as an error in the call reports it: the line
-- of the token its arguments open with (5.1 `funcargs`, 5.4 on), or of the
-- expression the chain of calls starts with (5.2, 5.3)
local ld = loadstring or load
local function try(src)
  print(pcall(assert(ld(src, "=c"))))
end
try("return (function()\n  return nil\nend)()()")
try("local g = function() end\nreturn g{\n}()")
try("local o = {m = function() end}\nreturn o:m(\n1\n)()")
try("local o = {m = function() end}\nreturn o:m{\n}:m()")
try("local s = setmetatable({}, {__call = function() end})\nreturn s'a\\\nb'()")
