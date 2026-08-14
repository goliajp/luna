# -*- coding: utf-8 -*-
"""Colour the docs code blocks the way kevy's CodeBlock does.

kevy paints in the browser; this site has no JavaScript, so the spans are
written into the HTML instead. The rules are kevy's, and deliberately
thin: a highlighter is a large dependency to make a few snippets slightly
prettier, and the things worth telling apart are comments and strings.
"""
import re, html, sys

# Keywords: kevy's list, plus the ones the languages here actually use.
RUST = r'\b(let|const|mut|fn|use|pub|impl|struct|enum|trait|match|async|await|move|return|if|else|for|while|loop|new)\b'
LUA  = r'\b(local|function|end|return|if|then|else|elseif|for|in|do|while|repeat|until|nil|true|false|and|or|not)\b'

RULES = {
    # (comment, string, keyword|None)
    'rust': (r'//[^\n]*', r'"[^"]*"|\'[^\']*\'', RUST),
    'lua':  (r'--[^\n]*', r'"[^"]*"|\'[^\']*\'', LUA),
    'sh':   (r'#[^\n]*',  r'"[^"]*"|\'[^\']*\'', None),
}

def detect(code):
    if re.search(r'^\s*(luna|cargo|rustup|docker|sudo|curl|\$)\b', code, re.M): return 'sh'
    if re.search(r'^\s*(FROM|RUN|COPY|CMD|ENTRYPOINT)\b', code, re.M): return 'sh'
    if re.search(r'^\s*\[[\w.-]+\]\s*$|^\s*[\w-]+\s*=\s*[\["\d]', code, re.M) \
       and not re.search(r'\b(fn|let|use)\b', code): return 'sh'
    if re.search(r'\b(local|function)\b', code) and not re.search(r'\b(fn|let|impl)\b', code): return 'lua'
    return 'rust'

def paint(code, lang):
    comment, string, kw = RULES[lang]
    pat = f'({comment})|({string})' + (f'|({kw})' if kw else '')
    out, last = [], 0
    for m in re.finditer(pat, code):
        out.append(html.escape(code[last:m.start()], quote=False))
        cls = 'tok-c' if m.group(1) else 'tok-s' if m.group(2) else 'tok-k'
        out.append(f'<span class="{cls}">{html.escape(m.group(0), quote=False)}</span>')
        last = m.end()
    out.append(html.escape(code[last:], quote=False))
    return ''.join(out)

dry = '--dry' in sys.argv
for p in ['docs.html', 'zh/docs.html', 'ja/docs.html']:
    s = open(p, encoding='utf-8').read()
    seen = []
    def go(m):
        # Strip any spans from a previous run first, or they would be read
        # back as source and escaped into the page as literal text.
        raw = html.unescape(re.sub(r'</?span[^>]*>', '', m.group(1)))
        lang = detect(raw)
        seen.append((lang, raw.strip().split('\n')[0][:52]))
        return f'<pre><code>{paint(raw, lang)}</code></pre>'
    new = re.sub(r'<pre><code>(.*?)</code></pre>', go, s, flags=re.S)
    if not dry:
        open(p, 'w', encoding='utf-8').write(new)
    if p == 'docs.html':
        for lang, first in seen:
            print(f'  {lang:5} │ {first}')
    print(f'{p}: {len(seen)} blocks' + ('' if dry else ' painted'))
