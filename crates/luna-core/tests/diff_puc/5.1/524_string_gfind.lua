-- v2.15 P2.5 (5.1): string.gfind = string.gmatch alias.
local words = {}
for w in string.gfind("hello world foo", "%w+") do
  words[#words+1] = w
end
print(table.concat(words, ","))
