-- v2.15 P2.5 (5.1): getfenv returns global env by default.
print(getfenv() == _G)
