-- v2.12 CORPUS-III: os.date with UTC ("!") prefix + fixed
-- epoch inputs only — deterministic across machines.
print(os.date("!%Y-%m-%d %H:%M:%S", 0))
print(os.date("!%Y-%m-%d %H:%M:%S", 86400))
print(os.date("!%Y-%m-%d %H:%M:%S", 1000000000))
print(os.date("!%d/%m/%y", 946684800))
