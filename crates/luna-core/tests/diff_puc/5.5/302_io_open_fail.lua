-- v2.14 CV.1: io.open failure shape — nil + message + errno
-- (message text carries a path, so only structure is compared).
local f, err, code = io.open("/nonexistent_dir_xyz/f.txt", "r")
print(f == nil, type(err), type(code))
local ok = io.open("/nonexistent_dir_xyz/f.txt", "w")
print(ok == nil)
print((pcall(io.open)))
