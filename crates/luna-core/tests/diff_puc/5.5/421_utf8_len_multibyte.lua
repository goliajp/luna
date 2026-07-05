-- v2.15 P2.4 utf8: len on multibyte strings.
print(utf8.len("café"))       -- 4 (not 5)
print(utf8.len("©"))           -- 1 (2-byte encoding)
print(utf8.len(utf8.char(0x1F600) .. "!"))   -- 2
