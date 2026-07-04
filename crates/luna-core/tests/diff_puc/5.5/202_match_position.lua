-- v2.13 CORPUS-IV: position captures () in match/gmatch/gsub.
print(string.match("hello", "()ll()"))
print(string.match("abc", "()"))
print(string.find("abc", "()b()"))
for p in string.gmatch("a,b,,c", "()," ) do io.write(p, " ") end
print()
print(string.gsub("abc", "()", "%1"))
