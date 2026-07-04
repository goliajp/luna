-- v2.14 CV.3: packsize arithmetic.
print(string.packsize("<i4i8"))
print(string.packsize(">bhi4d"))
print(string.packsize("!4bxi4"))
print(string.packsize(""))
print(pcall(string.packsize, "s4"))
print(pcall(string.packsize, "z"))
