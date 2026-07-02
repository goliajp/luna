-- v2.12 CORPUS-III: os.date extra format specifiers (UTC,
-- fixed epoch) + os.difftime.
print(os.date("!%j", 946684800))
print(os.date("!%H:%M", 3661))
print(os.date("!%w", 0))
print(os.difftime(100, 50), os.difftime(50, 100))
print(math.type(os.difftime(100, 50)))
