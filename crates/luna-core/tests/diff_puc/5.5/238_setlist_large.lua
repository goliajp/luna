-- v2.13 CORPUS-IV: large table constructor (SETLIST batching
-- beyond one instruction's field window).
local t = {}
do
  local src = {}
  for i = 1, 300 do src[i] = i end
  t = src
end
print(#t, t[1], t[256], t[300])
local big = {
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
  21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38,
  39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, (function() return 51, 52 end)(),
}
print(#big, big[50], big[52])
