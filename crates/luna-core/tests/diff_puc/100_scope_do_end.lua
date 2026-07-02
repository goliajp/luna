-- v2.11 CORPUS-II: do-end block scope.
do
  local secret = "hidden"
  print(secret)
end
-- 'secret' NOT visible here
print(rawget(_ENV, "secret"))  -- nil
