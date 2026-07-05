-- v2.14 HD 5.2 seed: rawlen arrives.
print(rawlen({ 1, 2, 3 }), rawlen("hello"))
print(rawlen(setmetatable({ 1 }, { __len = function() return 99 end })))
print(#setmetatable({ 1 }, { __len = function() return 99 end }))
