-- rule: liolib.io_open.1 liolib.newfile.1
-- A failed io.open leaves behind the handle the library made before
-- trying: what the failures allocate, with the collector stopped (the loop
-- run once first, so the stack has grown as far as it takes it)
collectgarbage()
collectgarbage()
collectgarbage("stop")
local before = 0
for round = 1, 2 do
  before = collectgarbage("count")
  for i = 1, 40 do
    local f, e = io.open("/nonexistent-dir-luna/file")
  end
end
print("open", (collectgarbage("count") - before) * 1024)
