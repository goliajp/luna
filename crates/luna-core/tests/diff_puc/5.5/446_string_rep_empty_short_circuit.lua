-- v2.18 P3: string.rep must short-circuit when the piece is empty.
--
-- With both the string and the separator empty, the result is "" for any
-- n, but a naive implementation loops n times copying zero bytes. The
-- size guard does not catch it (0 * n never overflows), so
-- string.rep("", math.maxinteger, "") hangs the VM outright — a DoS for
-- any embedder running untrusted Lua.
--
-- PUC fixed this in 5.5.1 (lstrlib.c:144, `n <= 0 || (len | lsep) == 0`);
-- 5.5.0 and earlier hang. luna hung identically until v2.18.
--
-- Reachable only with a huge n, which is why 500 fixtures never caught
-- it: every existing rep fixture uses a small count.
print(#string.rep("", math.maxinteger, ""))
print(#string.rep("", math.maxinteger))
print(#string.rep("", 1 << 40, ""))
-- separator non-empty, string empty: piece is non-zero, normal path
print(string.rep("", 3, "-"))
-- string non-empty, separator empty
print(string.rep("a", 3, ""))
print(string.rep("a", 3, "-"))
-- degenerate counts still behave
print(#string.rep("", 0, ""))
print(#string.rep("ab", 0))
print(#string.rep("", 1, ""))
-- The size guard must still fire for a genuinely huge result. Only the
-- boolean is compared: the message carries a chunkname and line number,
-- which the harness wraps differently on each side ("stdin:N" vs
-- "eval:M"), so error text is not comparable here.
local ok = pcall(function() return #string.rep("a", math.maxinteger) end)
print(ok)
