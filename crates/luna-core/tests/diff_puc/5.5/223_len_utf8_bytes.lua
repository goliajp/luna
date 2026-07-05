-- v2.13 CORPUS-IV: # is BYTE length; utf8.len is codepoint count.
local s = "héllo"
print(#s, utf8.len(s))
local zh = "中文字"
print(#zh, utf8.len(zh))
print(#"", utf8.len(""))
local bad = "\xFF\xFE"
print(#bad, utf8.len(bad))
print(utf8.len(zh, 4))
