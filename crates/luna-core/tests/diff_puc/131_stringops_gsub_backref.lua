-- v2.12 CORPUS-III: gsub backref + repetition.
print(string.gsub("abcabc", "(a)(b)(c)", "%3%2%1"))
print(string.gsub("hello world", "(%w+) (%w+)", "%2 %1"))
print(string.gsub("aaaa", "aa", "<X>"))
-- limit=0 means no replacement
print(string.gsub("aaa", "a", "b", 0))
