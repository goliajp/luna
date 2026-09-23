-- v3.1 W2: PUC 5.4.9 moved the pattern matcher's depth counter reset from
-- prepstate to reprepstate. Before that, an error raised deep inside one
-- gmatch step (here: a malformed trailing '%' reached after 150 nested
-- 'a?' items) left the counter where the error unwound it, so the next
-- call on the same iterator reported "pattern too complex" instead of the
-- real error. 5.4.8 prints the wrong message on the second line; 5.4.9
-- and luna print the malformed-pattern error both times.
local pat = string.rep("a?", 150) .. "b%"
local it = string.gmatch(string.rep("a", 150) .. "b", pat)
print(pcall(it))
print(pcall(it))
-- A fresh gmatch and string.find each start from a clean counter.
print(pcall(string.gmatch(string.rep("a", 150) .. "b", pat)))
print(pcall(string.find, string.rep("a", 150) .. "b", pat))
