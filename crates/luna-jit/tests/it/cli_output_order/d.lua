for i = 1, 3 do print(string.rep(string.char(96 + i), 3000)) end
io.stderr:write("MID\n")
for i = 1, 3 do print(i) end
error("big")
