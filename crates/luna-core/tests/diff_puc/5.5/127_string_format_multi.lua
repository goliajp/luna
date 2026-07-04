-- v2.12 CORPUS-III: string.format multi-arg.
print(string.format("%s=%d, %s=%d", "x", 10, "y", 20))
print(string.format("[%d,%d,%d]", 1, 2, 3))
print(string.format("%d%% done", 50))
-- excess args ignored, missing args error
print(string.format("%d", 42, "extra"))
