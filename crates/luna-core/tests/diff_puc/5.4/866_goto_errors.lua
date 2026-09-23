-- goto/label errors come from the parser at fixed points — when a label
-- is placed, when a block or function closes — and are reported at the
-- line the parser stands on there; they win over a syntax error further
-- down. The three generations (5.2/5.3, 5.4, 5.5) differ in when.
local ld = loadstring or load
local function e(src)
  local f, msg = ld(src, "=c")
  print(f and "ok" or msg)
end
e("goto a\nlocal x = 1\n::a::\nreturn x")
e("do\n  goto a\n  local x\n  ::a::\n\n  print(x)\nend")
e("local function f()\n  goto nope\nend\n\nreturn 1")
e("local function f()\n  goto nope\nend\nx = = 1")
e("goto a\ngoto b")
e("::a::\n\n\n::a::")
e("::a:: do ::a:: end")
e("::a:: ::a:: x = = 1")
e("do goto a; local x = 1 ::a:: ; ; end return 1")
e("goto f local function f() end ::f:: return 1")
e("for i = 1, 3 do if i == 2 then goto continue end local x = i ::continue:: print(x) end")
e("do ::a:: end goto a")
