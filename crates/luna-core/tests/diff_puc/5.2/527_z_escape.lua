-- v2.14 HD 5.2 seed: the \z escape skips following whitespace.
local s = "hello \z
           world"
print(s)
print("a\z
b")
print(#"x\z  y")
