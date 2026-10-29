-- long names and long string constants of several functions in one chunk.
-- Up to 5.2 each function's table keys the scanner's strings, so each
-- function has its own copy of a long one, and a prototype keeps the copy of
-- each name of its locals and each constant it has when the function ends
-- and its table goes; from 5.3 the chunk's one table keys one copy for all.
-- The count is read after each load, with a whole cycle at every step in
-- it, and after the collections that follow
local ld = loadstring or load
-- 5.4's lua.c starts the collector in generational mode
pcall(collectgarbage, "incremental")
local name = "a_parameter_with_a_name_longer_than_forty_chars"
local text = "'a string constant that is longer than forty characters'"
local global = "a_global_name_that_is_longer_than_forty_characters"
local cases = {
  "local function f(" .. name .. ") end local a, b, c = 's1', 's2', 's3' return f",
  "local function f(" .. name .. ") end local function g(" .. name .. ") end return f, g",
  "local function f(" .. name .. ") end local x = " .. name .. " return f",
  "local " .. name .. " = 1 local function f() return " .. name .. " end return f",
  "local function f() local s = " .. text .. " end return " .. text,
  "local function f() return " .. text .. " end local function g() return " .. text .. " end return f, g",
  "local function f() return " .. global .. " end return " .. global,
  "local " .. name .. " = 1 return " .. name .. ", " .. name,
  "local function f() local function g(" .. name .. ") return " .. text .. " end return g end return f",
}
for i, src in ipairs(cases) do
  for _, stepping in ipairs({false, true}) do
    -- the pause takes effect at the end of a cycle
    if stepping then
      collectgarbage("setpause", 0); collectgarbage("setstepmul", 1000000)
    end
    collectgarbage(); collectgarbage()
    if not stepping then collectgarbage("stop") end
    local before = collectgarbage("count")
    local fn = ld(src)
    local loaded = collectgarbage("count")
    collectgarbage("restart"); collectgarbage(); collectgarbage()
    local kept = collectgarbage("count")
    collectgarbage("setpause", 200); collectgarbage("setstepmul", 200)
    print(i, stepping, string.format("%.0f %.0f", (loaded - before) * 1024, (kept - before) * 1024), fn ~= nil)
    fn = nil
  end
end
