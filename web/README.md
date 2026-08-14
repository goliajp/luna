# luna.golia.jp — website

Static site for **luna**, a GOLIA Lab project. No build step, no
framework, no JavaScript, no server-side logic — plain HTML and one
stylesheet. Serve this directory as the web root.

## Layout

```
web/
├── index.html          # English landing        → /
├── docs.html           # English docs           → /docs.html
├── zh/
│   ├── index.html      # 简体中文 landing        → /zh/
│   └── docs.html       # 简体中文 docs           → /zh/docs.html
├── ja/
│   ├── index.html      # 日本語 landing          → /ja/
│   └── docs.html       # 日本語 docs             → /ja/docs.html
└── assets/
    ├── style.css       # one stylesheet, shared by all six pages
    ├── luna-logo.svg   # the mark
    └── golia-wordmark.png
```

English is the default locale at the root; `zh/` and `ja/` mirror it.
Every page carries `<link rel="alternate" hreflang="…">` pointing at
`https://luna.golia.jp/` (en), `/zh/` and `/ja/`, with `x-default → /`.
The in-page switcher links between the three.

## Design

A **Golia Lab** project page, set like a journal article: warm paper
ground (`#fcfbf8`), ink black text, hairline rules, generous measure.
GOLIA blue (`#155dfc`) is structural rather than decorative — it marks
section numbers, key figures, and the one link state.

Type: **Archivo** for display, **IBM Plex Sans** for prose, **IBM Plex
Mono** for every number and identifier. CJK falls back to the system UI
faces rather than Mincho, so the page reads as modern in all three
languages. `line-break: strict` and `word-break: auto-phrase` let the
browser do correct CJK typesetting natively.

**The stylesheet is kevy.golia.jp's, not a version of it** — every rule
here appears there verbatim, and every class this site uses is one that
site defines. The masthead and footer markup is likewise identical, with
only luna's own identity substituted: the logo, the wordmark, the nav
labels, and the three links (luna has no npm package, so kevy's fourth
footer link is absent). The sites should read as one lab rather than
three unrelated projects. If you change any of it here, change it there
too — the family resemblance is the thing that breaks first.

What is absent is as deliberate as what is present: kevy's terminal,
calculator, bar charts, tabs and recipe blocks have no counterpart here,
so their rules were not carried over.

There is no theme toggle and no dark variant: the paper ground *is* the
design, and a dark inversion of it would be a different design wearing
the same layout.

## Deploy

```sh
./deploy.sh --check     # verify without uploading
./deploy.sh             # verify, upload, then confirm the origin
```

The script rsyncs this directory to `t01:/apps/luna/web` and checks that
the origin actually serves the version in the tree — a stale deploy shows
up nowhere else. It verifies before uploading, because a static site
fails silently: a broken tag or a missing asset still returns HTTP 200.
The checks are tag balance, every class having a rule behind it, every
relative reference resolving, and one version string across all six pages.

The box, TLS and the Caddy vhost belong to `goliajp/devops`; the script
never touches them. It does not run `caddy deploy` — that regenerates the
whole Caddyfile from CaddyStore and drops any site not in the store.

What ships differs from this directory in one way: the stylesheet is
renamed after its own contents (`style.<hash>.css`) and the pages are
rewritten to match, so a returning visitor's cached copy can never win.
The source tree keeps the plain `style.css` and stays previewable as-is.

Local preview: `cd web && python3 -m http.server 8080`.

## Two passes over the pages

Both are idempotent and both hold `<pre>`, `<code>` and every tag out of
what they touch. Run them after editing content; `--dry` reports without
writing.

```sh
cd web && python3 paint.py && python3 punct.py
```

**`paint.py`** writes the syntax-highlight spans into the docs pages.
kevy does the same colouring in the browser, so its prerendered HTML has
none; this site has no JavaScript, so the spans live in the markup. It
re-detects each block's language, and strips the previous run's spans
before re-reading the source.

**`punct.py`** gives the Chinese and Japanese pages their own
punctuation. A half-width comma in a CJK sentence sets with the wrong
sidebearings and reads as a typo. Two things it gets right that a
search-and-replace would not: a bracket can straddle markup — the two
halves of `(<code>luna-core</code> では …)` live in different text nodes
— and what decides a mark is the sentence it sits in, not the character
before it, so the comma in `… C ABI, luna-core 则是 …` is converted even
though a Latin letter precedes it. It leaves alone thousands separators,
and brackets whose contents are code rather than prose.

Neither tool is uploaded.

## External dependencies

The pages load web fonts from Google Fonts (`fonts.googleapis.com` /
`fonts.gstatic.com`): **Archivo**, **IBM Plex Sans** and **IBM Plex
Mono**. Under a Content-Security-Policy, allow those two hosts — or
self-host the fonts and rewrite the `<link>` in each `<head>` for zero
third-party requests. Nothing else is fetched: no analytics, no
trackers, no scripts at all.

## Updating for a new release

The version string `v3.0.0` appears in the `.eyebrow` of all six pages.
Bump them on release — `deploy.sh` refuses to ship a tree where the six
disagree, which is what a half-finished bump looks like. This is also
step 8 of `.dev/rfcs/monthly-drift-sweep.md`.

Content is snapshotted against the crate docs under `../docs/`. Re-sync
when the embedding API or the dialect matrix changes. The landing page
also states two measured figures — the fixture count and the cold-start
heap — which should be checked against reality rather than carried
forward on faith.

---

Built and maintained by GOLIA K.K. · luna is dual-licensed MIT / Apache-2.0.
