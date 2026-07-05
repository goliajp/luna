-- v2.14 CV.1: os.getenv on an unset variable.
print(os.getenv("LUNA_DEFINITELY_NOT_SET_XYZ_12345"))
print(type(os.getenv("PATH")))
