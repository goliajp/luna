-- v2.15 P2.4 utf8: codepoint extraction.
print(utf8.codepoint("A"))          -- 65
print(utf8.codepoint("Hi", 1, 2))   -- 72 105
print(utf8.codepoint("é"))          -- 233
print(utf8.codepoint(utf8.char(0x1F600), 1))
