-- v2.14 HD 5.1 seed: string.gmatch + the 5.0-compat gfind alias
-- present in the official 5.1 binary.
for w in string.gmatch("one two", "%a+") do io.write(w, "|") end
print()
print(type(string.gfind))
for w in string.gfind("a b", "%a+") do io.write(w, ";") end
print()
