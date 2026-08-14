#!/usr/bin/env python3
"""Give the Chinese and Japanese pages their own punctuation.

Prose typed on a Latin keyboard arrives with Latin marks in it, and a
half-width comma in a CJK sentence sets with the wrong sidebearings — it
reads as a typo to anyone the page is written for.

Two things make this harder than a search-and-replace:

  * Code is not prose. Punctuation inside <pre> and <code> is syntax, and
    a full-width bracket there would be a bug. Those spans are held out.

  * A bracket can straddle markup — `(<code>luna-core</code> では …)` has
    its two halves in different text nodes, so matching within a single
    node converts one half and leaves the other. The pass therefore works
    on the whole document against a mask of which offsets are prose, and
    pairs brackets across the tags between them.

Idempotent: the marks it writes are not ones it matches. Run it after
editing any CJK page.

    python3 punct.py            # rewrite in place
    python3 punct.py --dry      # report only
"""
import re
import sys

CJK = '[一-鿿぀-ヿ㐀-䶿]'

# Chinese takes the full-width comma; Japanese the ideographic one.
MARKS = {
    'zh': {',': '，', ';': '；', ':': '：', '!': '！', '?': '？'},
    'ja': {',': '、', ';': '；', ':': '：', '!': '！', '?': '？'},
}
# After these, a Latin mark is still wrong — the run has not left CJK.
TRAILING = '，、；：！？）。」』'

PAGES = [('zh/docs.html', 'zh'), ('zh/index.html', 'zh'),
         ('ja/docs.html', 'ja'), ('ja/index.html', 'ja')]


def prose_mask(s):
    """True at every offset that is prose rather than markup or code."""
    mask = bytearray(b'\x01') * len(s)
    for m in re.finditer(r'<pre\b.*?</pre>|<code\b.*?</code>|<[^>]+>', s, re.S):
        mask[m.start():m.end()] = b'\x00' * (m.end() - m.start())
    return mask


def convert(s, lang):
    table = MARKS[lang]
    mask = prose_mask(s)
    edits = {}          # offset -> replacement character

    # Brackets first: pair them across markup, and convert only the pairs
    # whose contents are CJK. `f(x)` in a sentence stays as it is.
    stack = []
    for i, ch in enumerate(s):
        if not mask[i]:
            continue
        if ch == '(':
            stack.append(i)
        elif ch == ')' and stack:
            open_at = stack.pop()
            inner = ''.join(s[j] for j in range(open_at + 1, i) if mask[j])
            if re.search(CJK, inner):
                edits[open_at], edits[i] = '（', '）'

    # Then the marks. What decides them is the sentence they are in, not the
    # character before them: `… Cranelift JIT 与 C ABI, luna-core 则是 …` is a
    # Chinese sentence, and its comma belongs to Chinese even though a Latin
    # letter happens to precede it.
    visible = [i for i in range(len(s)) if mask[i]]
    pos = {i: n for n, i in enumerate(visible)}
    text = ''.join(s[i] for i in visible)

    for m in re.finditer(r'[,;:!?]', s):
        i = m.start()
        if not mask[i]:
            continue
        # A thousands separator is not punctuation: 44,518 stays as it is.
        if m.group(0) == ',' and re.match(r'\d', s[i - 1:i]) and re.match(r'\d', s[i + 1:i + 2]):
            continue
        n = pos[i]
        # The sentence around it, bounded by CJK full stops and line breaks.
        left = max((text.rfind(c, 0, n) for c in '。\n！？'), default=-1)
        right = min((r for c in '。\n！？' if (r := text.find(c, n + 1)) != -1),
                    default=len(text))
        if re.search(CJK, text[left + 1:right]):
            edits[i] = table[m.group(0)]

    if edits:
        out = list(s)
        for i, ch in edits.items():
            out[i] = ch
        s = ''.join(out)
    s, n = drop_heading_stops(s)
    return s, len(edits) + n


def drop_heading_stops(s):
    """A CJK heading does not end in a full stop.

    Chinese and Japanese typographic practice: a title, a label or a
    standalone phrase carries no 。 — the line break already ends it, and
    the stop makes a heading read as a stray sentence. A question or
    exclamation mark stays, because those carry tone rather than closure.
    """
    n = 0

    def strip(m):
        nonlocal n
        open_tag, body, close = m.group(1), m.group(2), m.group(3)
        stripped = re.sub(r'。(\s*)$', r'\1', body)
        if stripped != body:
            n += 1
        return open_tag + stripped + close

    # Headings, and the short label elements that are phrases by construction.
    s = re.sub(r'(<h[1-4]\b[^>]*>)(.*?)(</h[1-4]>)', strip, s, flags=re.S)
    s = re.sub(r'(<div class="(?:k|eyebrow)">)(.*?)(</div>)', strip, s, flags=re.S)
    return s, n


def main():
    dry = '--dry' in sys.argv
    total = 0
    for path, lang in PAGES:
        s = open(path, encoding='utf-8').read()
        new, n = convert(s, lang)
        total += n
        if not dry and n:
            open(path, 'w', encoding='utf-8').write(new)
        print(f'{path}: {n} mark{"" if n == 1 else "s"}'
              f'{" (dry run)" if dry else " converted" if n else ""}')
    return 0 if (dry or total == 0) else 0


if __name__ == '__main__':
    sys.exit(main())
