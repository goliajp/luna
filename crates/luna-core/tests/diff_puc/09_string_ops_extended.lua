-- v2.10 CORPUS: string library extended.
print(string.byte("hello", 2))
print(string.byte("hello", -1))  -- 'o' from end
print(string.char(72, 105))
print(string.sub("hello", -3))
print(string.sub("hello", 1, -2))
print(string.gmatch("a,b,c,d", "[^,]+")())
local t = {}
for w in string.gmatch("a,b,c,d", "[^,]+") do
  t[#t+1] = w
end
print(table.concat(t, "|"))
print(string.match("hello 42 world 100", "(%a+) (%d+) (%a+)"))
local n, k = string.gsub("hello", "l", "L")
print(n, k)
