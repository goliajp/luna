-- v2.15 P2.4 utf8: charpattern iteration.
local s = "abc"
local pieces = {}
for p in string.gmatch(s, utf8.charpattern) do
  pieces[#pieces+1] = p
end
print(table.concat(pieces, "|"))
print(#pieces)
