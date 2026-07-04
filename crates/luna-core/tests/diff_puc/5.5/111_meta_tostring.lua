-- v2.12 CORPUS-III: __tostring on tostring() and print.
local V = {}
V.__tostring = function(v) return "V<" .. v.id .. ">" end
local a = setmetatable({id=1}, V)
print(tostring(a))
print(a)      -- print calls tostring via __tostring
print(tostring(a) == "V<1>")

-- also fires for string.format %s
print(string.format("%s|", a))
