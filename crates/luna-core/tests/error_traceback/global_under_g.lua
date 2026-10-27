local e = error
error = nil
for i = 1, 32 do _G['v' .. i] = i end
v4 = e
string.gsub('x', 'x', e)
