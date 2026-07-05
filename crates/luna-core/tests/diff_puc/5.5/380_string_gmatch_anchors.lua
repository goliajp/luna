-- v2.14 CV.3: gmatch with position captures and %f frontier.
for pos, word in ("one two"):gmatch("()(%a+)") do io.write(pos, "=", word, " ") end
print()
for w in ("THE (quick) brOwn"):gmatch("%f[%a]%u+%f[%A]") do io.write(w, "|") end
print()
