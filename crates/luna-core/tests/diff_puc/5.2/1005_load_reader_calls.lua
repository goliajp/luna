-- how often `load` calls its reader function: the parser stops calling it
-- at a syntax error, and asks for the next piece as soon as it moves past
-- the end of the one it has
local function count(title, pieces)
  local i, calls = 0, 0
  local f, err = load(function()
    calls = calls + 1
    i = i + 1
    return pieces[i]
  end, '=s')
  print(title, calls, f and 'ok' or err)
end
count('whole', {'local x', ' = 1', '0 return x', '+1'})
count('error first', {'x = = 1\n', 'print(1)\n', 'return 2'})
count('error at piece end', {'x = )', 'print(1)', 'return 2'})
count('numeral', {'local 1', 'abc = 2', 'return 3'})
count('string', {"x = 'abc", 'def', "\n'", 'return 1'})
count('break', {'break\n', 'x = 1\n', 'return x'})
count('empty piece', {'return', '', ' 1'})
count('bytes', {'r', 'e', 't', 'u', 'r', 'n', ' ', '1', ',', ',', '2'})
