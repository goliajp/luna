# luna.golia.jp — website

Static marketing + documentation site for **luna**, a GOLIA product. No
build step, no framework, no server-side logic — plain HTML/CSS/JS. Just
serve this directory as the web root.

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
    ├── style.css       # one stylesheet, shared by all 6 pages
    └── app.js          # theme toggle, mobile nav, copy buttons, scrollspy, tabs
```

English is the default locale at the root; `zh/` and `ja/` mirror it. Every
page carries `<link rel="alternate" hreflang="…">` tags pointing at
`https://luna.golia.jp/` (en), `/zh/`, and `/ja/`, with `x-default → /`. The
in-page language switcher links between the three locales.

## Design identity

Deep-violet base with a gold accent — violet for the brand/tech signal, gold
for the financial/value note — on a fine data grid, with tabular-figure
numerics and glass panels for a precision-instrument, fintech feel. Type:
**Bricolage Grotesque** display, **Hanken Grotesk** body, **JetBrains Mono**
for code. The luna wordmark carries a small `by GOLIA` tag; the moon glyph is
a violet orb with a gold rim-light. Dark theme is default; a light theme
(lavender paper) ships via the nav toggle.

## Deploy

It's a static bundle — copy `web/` to the document root and point the vhost
at it. Nothing to compile.

Minimal nginx sketch:

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

Optional: a first-visit `Accept-Language` redirect from `/` to `/zh/` or
`/ja/`. Not required — the switcher and `hreflang` already cover discovery,
and defaulting everyone to English is fine.

Local preview: `cd web && python3 -m http.server 8080`, then open
`http://localhost:8080/`.

## External dependencies

The pages load web fonts from Google Fonts (`fonts.googleapis.com` /
`fonts.gstatic.com`): **Bricolage Grotesque** (display), **Hanken Grotesk**
(Latin body), **JetBrains Mono** (code), and **Noto Sans/Serif SC & JP** for
the Chinese and Japanese pages. If a Content-Security-Policy is applied,
allow those two hosts — or self-host the fonts and rewrite the `<link>` tags
in each `<head>` for zero third-party requests. Everything else (theme, i18n
switch, copy buttons) is inline and self-contained; there are no analytics or
trackers.

## Theme & i18n

- **Theme** — light/dark toggle in the nav; the choice persists in
  `localStorage` (`luna-theme`) and is applied before first paint by a tiny
  inline script in each `<head>` (no flash). Dark is the default.
- **Language** — separate HTML per locale (good for SEO and clean URLs), not
  a client-side string swap. Adding or editing a locale means copying a
  locale folder and translating the text nodes; the markup, CSS, and JS are
  identical across all six pages.

## Updating for a new release

The version string `v2.18.0` appears in each landing page's hero badge and
each docs page's intro sentence. Bump those on release. Content is
snapshotted against the crate docs under `../docs/` — re-sync if the
embedding API or dialect matrix changes.

---

Built and maintained by GOLIA K.K. · luna is dual-licensed MIT / Apache-2.0.
