for i = 1, 8 do _G['t' .. i] = {f = error} end
string.gsub('x', 'x', error)
