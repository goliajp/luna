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

**The masthead and footer are identical to tiktoken.golia.jp and
kevy.golia.jp by intent** — same markup, same classes, same GOLIA
wordmark in the footer. The three sites should read as one lab rather
than three unrelated projects. If you change either here, change it
there too, or the family resemblance is the thing that breaks.

There is no theme toggle and no dark variant: the paper ground *is* the
design, and a dark inversion of it would be a different design wearing
the same layout.

## Deploy

A static bundle — copy `web/` to the document root and point the vhost
at it. Nothing to compile.

```nginx
server {
    server_name luna.golia.jp;
    root /var/www/luna;          # this directory
    index index.html;

    # pretty /zh/ and /ja/ directory URLs resolve to their index.html
    location / {
        try_files $uri $uri/ $uri.html =404;
    }

    # long-cache the immutable assets
    location /assets/ {
        add_header Cache-Control "public, max-age=31536000, immutable";
    }
}
```

Local preview: `cd web && python3 -m http.server 8080`.

## External dependencies

The pages load web fonts from Google Fonts (`fonts.googleapis.com` /
`fonts.gstatic.com`): **Archivo**, **IBM Plex Sans** and **IBM Plex
Mono**. Under a Content-Security-Policy, allow those two hosts — or
self-host the fonts and rewrite the `<link>` in each `<head>` for zero
third-party requests. Nothing else is fetched: no analytics, no
trackers, no scripts at all.

## Updating for a new release

The version string `v3.0.0` appears in each landing page's kicker and
each docs page's kicker — six occurrences. Bump them on release; this is
also step 8 of `.dev/rfcs/monthly-drift-sweep.md`.

Content is snapshotted against the crate docs under `../docs/`. Re-sync
when the embedding API or the dialect matrix changes. The landing page
also states two measured figures — the fixture count and the cold-start
heap — which should be checked against reality rather than carried
forward on faith.

---

Built and maintained by GOLIA K.K. · luna is dual-licensed MIT / Apache-2.0.
