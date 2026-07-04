-- v2.14 CV.1: os.remove/os.rename failure shape (nil + msg +
-- errno; message carries paths so only structure is compared).
local ok, err, code = os.remove("/nonexistent_dir_xyz/file")
print(ok == nil, type(err), type(code))
local ok2, err2 = os.rename("/nonexistent_dir_xyz/a", "/nonexistent_dir_xyz/b")
print(ok2 == nil, type(err2))
