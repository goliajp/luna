#!/usr/bin/env bash
# Deploy the site to t01 (luna.golia.jp).
#
# The box, TLS and the Caddy vhost are owned by goliajp/devops — the vhost is
# a CaddyStore row rendered into t01's managed Caddyfile, and the DNS record
# is a `dns` row. Neither is ours to re-create here; this script only
# refreshes the static content those point at. In particular it never runs
# `caddy deploy`: that regenerates the whole Caddyfile from CaddyStore and
# silently drops any site not in the store (devops incident, powerme).
#
# There is no build step — the site is plain HTML and one stylesheet, so what
# ships is exactly what is in this directory.
#
#   usage: web/deploy.sh [--check]
#     --check   verify only; do not upload
set -euo pipefail

cd "$(dirname "$0")"

HOST=t01
ROOT=/apps/luna/web
URL=https://luna.golia.jp

check_only=false
[[ "${1:-}" == "--check" ]] && check_only=true

# ── verify before uploading ──────────────────────────────────────────
# A static site fails silently: a broken tag or a missing asset still
# serves HTTP 200. Check locally what the server cannot check for us.
echo "→ verifying local tree"
python3 - <<'PY'
from html.parser import HTMLParser
import re, glob, os, sys

VOID = {'meta','link','br','img','hr','input','source','path','circle','rect','use'}
css = open('assets/style.css', encoding='utf-8').read()
defined = set(re.findall(r'\.([a-zA-Z][\w-]*)', css))
pages = sorted(glob.glob('**/*.html', recursive=True))
assert len(pages) == 6, f"expected 6 pages, found {len(pages)}: {pages}"

versions = set()
bad = False
for f in pages:
    s = open(f, encoding='utf-8').read()

    class P(HTMLParser):
        def __init__(self): super().__init__(); self.st=[]; self.err=[]
        def handle_starttag(self, t, a):
            if t not in VOID: self.st.append(t)
        def handle_endtag(self, t):
            if t in VOID: return
            if not self.st: self.err.append(f"stray </{t}>"); return
            if self.st[-1] != t: self.err.append(f"</{t}> closes <{self.st[-1]}>")
            else: self.st.pop()
    p = P(); p.feed(s)

    used = set()
    for m in re.finditer(r'class="([^"]+)"', s): used.update(m.group(1).split())
    # Two kinds of class name are carried deliberately without a rule behind
    # them, and flagging either would train us to ignore this check:
    #   lucide*  — the icon set stamps its own name on every glyph it ships;
    #              it identifies the icon, it is not a styling hook.
    #   note     — the neutral .callout variant. Only .callout.loss restyles
    #              anything, so the common case correctly has no rule.
    known = {c for c in used if c.startswith('lucide')} | {'note'}
    unstyled = sorted(used - defined - known)

    refs = re.findall(r'(?:src|href)="((?!http|#|mailto:|/)[^"]+)"', s)
    missing = [r for r in refs
               if not os.path.exists(os.path.normpath(
                   os.path.join(os.path.dirname(f) or '.', r.split('#')[0])))]

    versions.update(re.findall(r'v\d+\.\d+\.\d+', s))

    problems = []
    if p.st:       problems.append(f"unclosed {p.st[:3]}")
    if p.err:      problems.append(f"malformed {p.err[:2]}")
    if unstyled:   problems.append(f"unstyled {unstyled}")
    if missing:    problems.append(f"missing {missing}")
    if problems:
        bad = True
        print(f"  ✗ {f}: {'; '.join(problems)}")
    else:
        print(f"  ✓ {f}")

# One version string across all six pages, or the release bump was partial.
if len(versions) != 1:
    bad = True
    print(f"  ✗ inconsistent version strings across pages: {sorted(versions)}")
else:
    print(f"  ✓ version {versions.pop()} consistent across all pages")

sys.exit(1 if bad else 0)
PY

if $check_only; then
  echo "✓ local tree ok (--check: nothing uploaded)"
  exit 0
fi

echo "→ uploading to $HOST:$ROOT"
ssh "$HOST" "mkdir -p $ROOT"
# --delete keeps the target an exact mirror; without it, files removed here
# (an old stylesheet, a retired script) keep being served indefinitely.
rsync -a --delete ./ "$HOST:$ROOT/" \
  --exclude deploy.sh --exclude README.md

echo "→ verifying $URL"
for path in / /docs.html /zh/ /ja/ /zh/docs.html /ja/docs.html /assets/style.css; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "$URL$path")
  [[ "$code" == "200" ]] || { echo "  ✗ $path → $code"; exit 1; }
  echo "  ✓ $path"
done

# The version string is the one thing a stale deploy shows most obviously,
# so confirm the origin actually serves the version in this tree.
want=$(grep -om1 'v[0-9]\+\.[0-9]\+\.[0-9]\+' index.html)
got=$(curl -s "$URL/" | grep -om1 'v[0-9]\+\.[0-9]\+\.[0-9]\+' || true)
[[ "$got" == "$want" ]] || { echo "  ✗ origin serves $got, tree has $want"; exit 1; }

echo "✓ deployed — $URL ($want)"
