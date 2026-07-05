-- v2.10 CORPUS: string patterns extended.
print(string.match("abc123def456", "%d+"))
print(string.match("abc123def456", "(%a+)(%d+)"))
for s in string.gmatch("hello world foo bar", "%a+") do io.write(s, " ") end
print()
print(string.find("abcxyzabc", "abc", 2))
print(string.find("hello", "^hel"))
print(string.match("  hello  ", "^%s*(.-)%s*$"))
print(string.gsub("aaa bbb ccc", "%s", "_"))
print(string.rep("abc", 3, "-"))
