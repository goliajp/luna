-- v2.13 CORPUS-IV: gmatch multi-captures + empty-match advance.
for k, v in string.gmatch("a=1,b=2,c=3", "(%w+)=(%w+)") do
  io.write(k, ":", v, " ")
end
print()
local n = 0
for _ in string.gmatch("abc", "x*") do n = n + 1 end
print(n)
for w in string.gmatch("  two  words ", "%S+") do io.write("[", w, "]") end
print()
