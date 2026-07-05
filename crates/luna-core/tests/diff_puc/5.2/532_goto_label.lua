-- v2.15 P2.5 (5.2): goto arrived.
local n = 0
::start::
n = n + 1
if n < 5 then goto start end
print(n)
