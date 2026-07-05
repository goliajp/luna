-- v2.12 CORPUS-III: string.char many args.
print(string.char(72, 101, 108, 108, 111, 44, 32, 87, 111, 114, 108, 100))
-- individual bytes
for i = 65, 70 do io.write(string.char(i)) end
print()
