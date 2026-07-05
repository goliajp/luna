-- v2.15 P2.5 (5.5): gsub with negated char class.
print(string.gsub("hello123world", "%A", "_"))     -- letters stay, others → _
print(string.gsub("abc  def", "%s+", " "))
print(string.gsub("a=1,b=2,c=3", "(%w)=(%d)", "%2:%1"))
