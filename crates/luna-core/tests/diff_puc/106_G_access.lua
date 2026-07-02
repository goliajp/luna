-- v2.12 CORPUS-III: _G is the environment table (chunk scope).
_G.marker = "hello"
print(marker)         -- hello
_G.marker = nil
print(marker)         -- nil

-- writes go to _G
foo = 42
print(_G.foo)
_G.foo = nil
