-- v2.14 CV.1: os.clock shape — float subtype, non-decreasing.
local a = os.clock()
local b = os.clock()
print(math.type(a), math.type(b))
print(a <= b)
print(a >= 0)
