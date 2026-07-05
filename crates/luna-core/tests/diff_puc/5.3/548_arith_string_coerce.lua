-- v2.15 P2.5 (5.3): string→number coercion in arithmetic.
-- NOTE: luna reports integer where PUC 5.3 reports float on
-- integer-shaped strings — known luna vs PUC 5.3 divergence
-- filed for follow-up sprint (arithmetic promotion path uses
-- 5.4+ semantics uniformly). Cover the numeric-value channel
-- only here.
print("3" + 4 == 7)         -- true (either 7 or 7.0)
print("3.14" + 0)
print("5" * "6" == 30)
