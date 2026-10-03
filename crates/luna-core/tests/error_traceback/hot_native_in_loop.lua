local t = {}
for i = 1, 1000 do t[i] = string.format("%d", i == 990 and {} or i) end
