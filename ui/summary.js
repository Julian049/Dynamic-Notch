// Last paragraph of Claude's final reply, as plain text: that is usually the result or the question it leaves.
// Skips code blocks and tables, turns a closing list into one line, and strips inline markdown.
function lastParagraph(md, max = 400) {
  const text = String(md || '')
    .replace(/\r/g, '')
    .replace(/```[\s\S]*?(```|$)/g, '\n\n'); // fenced code, even if unclosed
  const paragraphs = text
    .split(/\n\s*\n/)
    .map((p) => p.split('\n').filter((l) => !/^\s*\|/.test(l) && !/^\s*(-{3,}|\*{3,})\s*$/.test(l)))
    .filter((lines) => lines.some((l) => l.trim()));
  const last = paragraphs.pop();
  if (!last) return '';
  const plain = last
    .map((l) =>
      l
        .replace(/^\s*(#{1,6}\s+|>\s?|[-*+]\s+|\d+[.)]\s+)/, '')
        .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
        .replace(/(?<![\w*])(\*\*|__|\*|_|~~|`)(\S(?:.*?\S)?)\1(?![\w*])/g, '$2')
        .trim(),
    )
    .filter(Boolean)
    .join(last.length > 1 && /^\s*([-*+]|\d+[.)])\s/.test(last[0]) ? ' · ' : ' ');
  return plain.length > max ? `${plain.slice(0, max - 1).trimEnd()}…` : plain;
}

if (typeof module !== 'undefined') module.exports = { lastParagraph };
