-- rule: lcode.fixjump.1
-- A forward jump of exactly 2^24 instructions is the longest the sJ field
-- takes (PUC `MAXARG_sJ - OFFSET_sJ`); one more is "control structure too
-- long". Each `x = y` is two instructions.
local H = 1 << 23
for _, n in ipairs({ H, H + 1 }) do
  local f, e = load("if c then\n" .. string.rep("x = y\n", n) .. "end", "=c")
  print(n, f and "ok" or e)
end
