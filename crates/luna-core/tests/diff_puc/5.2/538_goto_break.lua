-- v2.15 P2.5 (5.2): goto used as break-out-of-inner-loop.
for i = 1, 3 do
  for j = 1, 3 do
    if i * j == 6 then
      io.write("stop@", i, ",", j, " ")
      goto done
    end
  end
end
::done::
print()
