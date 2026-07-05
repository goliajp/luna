-- v2.15 P2.5: collectgarbage("count") returns memory in KB.
collectgarbage("collect")
local mem_before = collectgarbage("count")
print(mem_before > 0)     -- true

-- allocate some memory
local buffer = {}
for i = 1, 1000 do buffer[i] = tostring(i) end
local mem_after = collectgarbage("count")
print(mem_after > mem_before)    -- true

-- release
buffer = nil
collectgarbage("collect")
local mem_final = collectgarbage("count")
print(mem_final < mem_after)    -- true
