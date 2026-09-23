-- v3.1 iopkg: the 5.2 os library. os.date checks its conversions against
-- the C99 set (E and O modifiers with their letters only) and reports the
-- rest of the format; a date gmtime cannot represent gives nil; os.time
-- returns a float and ignores a non-number field; os.execute gives
-- nil,"exit",code on failure; os.rename no longer names the file.
local function clean(s) return (tostring(s):gsub("'_G%.", "'"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local t0 = 86400 * 45 + 3600 * 15 + 61
p("date bad", pcall(os.date, "!%Q abc", t0))
p("date bad modifier", pcall(os.date, "!%Ez", t0))
p("date trailing", pcall(os.date, "!ab%", t0))
p("date modifiers", os.date("!%Ec|%EY|%Od|%OH|%Ey", t0))
p("date unrepresentable", os.date("!%Y", 2^62))
p("date float time", os.date("!%S", 1.9))
local ref = os.time{year = 2000, month = 1, day = 1, hour = 0}
p("time table field", os.time{year = 2000, month = 1, day = 2, hour = {}} - ref)
p("time missing", pcall(os.time, {year = 2000}))
p("difftime", os.difftime(10.5, 3), os.difftime(5))
p("execute", os.execute("exit 3"))
p("execute ok", os.execute("exit 0"))
local base = os.tmpname()
os.remove(base)
p("rename missing", os.rename(base, base .. "_to"))
p("exit table", pcall(os.exit, {}))
