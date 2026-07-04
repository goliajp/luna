-- v2.13 CORPUS-IV: %f frontier pattern.
print(string.gsub("THE (quick) fox", "%f[%a]%u+%f[%A]", "X"))
print(string.find("hello world", "%f[%w]%w+"))
local n = select(2, string.gsub("one two three", "%f[%w]", "|"))
print(n)
print(string.match("int x=10;", "%f[%d]%d+"))
