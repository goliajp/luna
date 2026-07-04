-- v2.14 CV.1: locale-independent numeric os.date specs (UTC,
-- fixed epochs): ISO week date %G/%V, %u, %e.
print(os.date("!%G-%V-%u", 946684800))
print(os.date("!%e|", 946684800))
print(os.date("!%C %y", 946684800))
print(os.date("!%D", 0))
print(os.date("!%T", 3661))
print(os.date("!%F", 1000000000))
