-- v2.18 P5/F1: errors naming a value read from a named-vararg table.
--
-- `function f(...t)` is 5.5 syntax; `t.k` compiles to VARGIDX rather than
-- GETFIELD (there is no table object to index -- the key arrives in a
-- register). luna's getobjname had no VARGIDX arm, so every error
-- mentioning such a value lost its "(field 'k')" suffix. PUC 5.5.1's
-- errors.lua added a case pinning exactly this.
--
-- Compares the message body only: the chunkname/line prefix is wrapped
-- differently on each side by the harness.
local function m(src)
  local f, e = load(src)
  if not f then print("LOADERR", (tostring(e):gsub("^.-:%d+: ", ""))) return end
  local ok, err = pcall(f)
  print((tostring(err):gsub("^.-:%d+: ", "")))
end
m("local function foo(...t) return t.xx + 1 end return foo()")
m("local function foo(...t) return t.xx .. 'a' end return foo()")
m("local function foo(...t) return #t.xx end return foo()")
m("local function foo(...t) return t.xx() end return foo()")
m("local function foo(...t) return t.xx.y end return foo()")
m("local function foo(...t) return -t.xx end return foo()")
m("local function foo(...t) return ~t.xx end return foo()")
-- the vararg table itself is a plain local
m("local function foo(...t) return t + 1 end return foo()")
-- a real key still resolves
m("local function foo(...t) return t.n + 1 end return foo(1,2)")
