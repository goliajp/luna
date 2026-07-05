-- v2.15 P2.4: long or/and chains.
print(nil or false or 0 or "found")
print(1 and 2 and 3 and 4 and 5)
print(1 and 2 or 3 and 4 or 5)
print(nil and nil and nil and 42)
print(true and false and true and true)
print((nil or 0) and (0 or nil) and "final")
