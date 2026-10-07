-- What seek reports on standard input while it is a pipe whose writer has
-- written nothing yet: run with the results file and a label, appended to
-- the file. Nothing is read, which would wait for the writer.
local out_name, label = ...
local out = assert(io.open(out_name, "a"))
local function r(...) local t = {} for i = 1, select("#", ...) do t[#t+1] = tostring((select(i, ...))) end return table.concat(t, ",") end
local function row(name, ...) out:write(label, "\t", name, "\t", r(...), "\n") end
row("stdin set0", io.stdin:seek("set", 0))
row("stdin cur", io.stdin:seek("cur"))
row("stdin end", io.stdin:seek("end"))
row("stdin set 1", io.stdin:seek("set", 1))
row("stdin cur again", io.stdin:seek("cur"))
row("stdin set -1", io.stdin:seek("set", -1))
out:close()
