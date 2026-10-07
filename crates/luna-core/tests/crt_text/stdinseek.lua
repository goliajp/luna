-- What seek reports on the standard streams: run with the results file and
-- a label, appended to the file.
local out_name, label = ...
local out = assert(io.open(out_name, "a"))
local function r(...) local t = {} for i = 1, select("#", ...) do t[#t+1] = tostring((select(i, ...))) end return table.concat(t, ",") end
local function row(name, ...) out:write(label, "\t", name, "\t", r(...), "\n") end
row("stdin set0", io.stdin:seek("set", 0))
row("stdin cur", io.stdin:seek("cur"))
row("stdin end", io.stdin:seek("end"))
row("stdin set0 again", io.stdin:seek("set", 0))
row("stdin read1", io.stdin:read(1))
row("stdin cur after read", io.stdin:seek("cur"))
row("stdin set0 after read", io.stdin:seek("set", 0))
row("stdin read1 again", io.stdin:read(1))
row("stdin set 1", io.stdin:seek("set", 1))
row("stdin readall", (io.stdin:read("*a") or ""):len())
row("stdin cur at end", io.stdin:seek("cur"))
row("stdin set -1", io.stdin:seek("set", -1))
io.stdout:write("out\n")
row("stdout cur", io.stdout:seek("cur"))
row("stdout set0", io.stdout:seek("set", 0))
row("stdout end", io.stdout:seek("end"))
io.stderr:write("err\n")
row("stderr cur", io.stderr:seek("cur"))
row("stderr set0", io.stderr:seek("set", 0))
row("stdin setvbuf no", io.stdin:setvbuf("no"))
row("stdin set0 after setvbuf", io.stdin:seek("set", 0))
row("stdin read1 after setvbuf", io.stdin:read(1))
out:close()
