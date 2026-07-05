-- v2.12 CORPUS-III: numeric-for loop variable subtype (5.3+):
-- all-integer bounds keep integer; any float bound floats all.
for i = 1, 1 do print(math.type(i)) end
for i = 1, 1, 1 do print(math.type(i)) end
for i = 1.0, 1.0 do print(math.type(i)) end
for i = 1, 2.0 do print(math.type(i)) break end
for i = 1, 2, 0.5 do print(math.type(i)) break end
