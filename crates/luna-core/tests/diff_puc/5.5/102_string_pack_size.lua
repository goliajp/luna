-- v2.12 CORPUS-III: string.packsize.
print(string.packsize("<Bi4"))     -- 5
print(string.packsize("<i8i8"))    -- 16
print(string.packsize("<c4"))      -- 4 (fixed-len string)
