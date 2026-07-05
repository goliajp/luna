-- v2.11 CORPUS-II: gmatch with captures.
local parts = {}
for a, b in string.gmatch("k1=v1,k2=v2,k3=v3", "(%w+)=(%w+)") do
  parts[#parts+1] = a .. ":" .. b
end
print(table.concat(parts, "|"))

-- single capture
local words = {}
for w in string.gmatch("apple, banana, cherry", "%a+") do
  words[#words+1] = w
end
print(table.concat(words, "/"))
