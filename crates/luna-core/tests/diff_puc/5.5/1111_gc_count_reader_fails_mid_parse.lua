-- a reader that fails part way through the source: what PUC's parser made
-- of the text read so far, a token half scanned included, stays as
-- garbage in the count (under pcall: no message handler runs on the error)
local function run(pieces)
  local i = 0
  local function reader()
    i = i + 1
    if pieces[i] then return pieces[i] end
    error("reader failed " .. i)
  end
  collectgarbage()
  collectgarbage("stop")
  local before = collectgarbage("count") * 1024
  local _, f, msg = pcall(load, reader, "=pieces")
  local after = collectgarbage("count") * 1024
  collectgarbage("restart")
  print(f, msg, after - before)
end
run({"local t = {1, 2, 3}\nlocal function f(a, b)\n  return a + b, 'str", "ing' .. tostring(a)\nend\nlocal x = f(1, 2)\n"})
run({"local s = 'abc'\nlocal u = s:upper()", "\nlocal function g()\n  local k = {x = 1, y = 2}\n  return k\nend\nre"})
run({"return 1"})
run({"x = 1\nlocal y = {", "a = 1, b = 2, c = 3, d = 4, e = 5, f = 6, g = 7, h = 8}\n"})
run({"-- a comment", " then more"})
