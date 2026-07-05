-- v2.14 CV.2: utf8.codepoint on an invalid byte sequence.
return utf8.codepoint("\xFF")
