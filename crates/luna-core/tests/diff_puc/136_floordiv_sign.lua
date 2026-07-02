-- v2.12 CORPUS-III: floor division + modulo sign rules.
-- PUC 5.4+: // floors toward negative infinity; % result has
-- the sign of the divisor.
print(7 // 2, -7 // 2, 7 // -2, -7 // -2)
print(7 % 3, -7 % 3, 7 % -3, -7 % -3)
print(7.0 // 2.0, -7.0 // 2.0, 7.5 // 2.0)
print(7.5 % 2.0, -7.5 % 2.0)
