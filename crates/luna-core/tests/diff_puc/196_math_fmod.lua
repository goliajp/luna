-- v2.13 CORPUS-IV: math.fmod sign follows DIVIDEND (C fmod);
-- % sign follows divisor — contrast both.
print(math.fmod(7, 3), 7 % 3)
print(math.fmod(-7, 3), -7 % 3)
print(math.fmod(7, -3), 7 % -3)
print(math.fmod(-7, -3), -7 % -3)
print(math.fmod(5.5, 2), 5.5 % 2)
print(math.type(math.fmod(7, 3)), math.type(math.fmod(7.0, 3)))
print((pcall(math.fmod, 1, 0)))
print(math.fmod(1.0, 0.0) ~= math.fmod(1.0, 0.0))
