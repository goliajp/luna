-- v2.14 CV.1: io.write value rendering — numbers use tostring
-- spelling, multiple args concatenate with no separator.
io.write("a", "b", 1, 2.5, "\n")
io.write(10, 20.0, "\n")
io.write("x")
io.write("y", "\n")
print("done")
