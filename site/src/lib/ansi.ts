// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

// Turns a `tmux capture-pane -e` screen (text plus SGR colour codes) into HTML spans. The
// terminal player runs it in the browser; still screens run it at build time. Trailing spaces
// were trimmed from the recordings, so each line is padded to the full width in the style that
// is active at its end (the style of the trimmed spaces).

const BASE16 = [
  '#3b4252', '#bf616a', '#a3be8c', '#ebcb8b', '#81a1c1', '#b48ead', '#88c0d0', '#e5e9f0',
  '#4c566a', '#bf616a', '#a3be8c', '#ebcb8b', '#81a1c1', '#b48ead', '#8fbcbb', '#eceff4',
];

interface Sgr { fg?: string; bg?: string; b?: boolean; d?: boolean; i?: boolean; u?: boolean; r?: boolean }

function x256(n: number): string {
  if (n < 16) return BASE16[n];
  if (n < 232) {
    const c = [0, 95, 135, 175, 215, 255];
    n -= 16;
    return `rgb(${c[Math.floor(n / 36)]},${c[Math.floor(n / 6) % 6]},${c[n % 6]})`;
  }
  const v = 8 + (n - 232) * 10;
  return `rgb(${v},${v},${v})`;
}

function apply(st: Sgr, params: string): void {
  const p = params ? params.split(';').map((x) => Number(x) || 0) : [0];
  for (let i = 0; i < p.length; i++) {
    const c = p[i];
    if (c === 0) { for (const k of Object.keys(st)) delete st[k as keyof Sgr]; }
    else if (c === 1) st.b = true;
    else if (c === 2) st.d = true;
    else if (c === 3) st.i = true;
    else if (c === 4) st.u = true;
    else if (c === 7) st.r = true;
    else if (c === 22) st.b = st.d = false;
    else if (c === 23) st.i = false;
    else if (c === 24) st.u = false;
    else if (c === 27) st.r = false;
    else if (c >= 30 && c <= 37) st.fg = BASE16[c - 30];
    else if (c >= 90 && c <= 97) st.fg = BASE16[c - 82];
    else if (c >= 40 && c <= 47) st.bg = BASE16[c - 40];
    else if (c >= 100 && c <= 107) st.bg = BASE16[c - 92];
    else if (c === 39) delete st.fg;
    else if (c === 49) delete st.bg;
    else if ((c === 38 || c === 48) && i + 1 < p.length) {
      const k = c === 38 ? 'fg' : 'bg';
      if (p[i + 1] === 5) { st[k] = x256(p[i + 2]); i += 2; }
      else if (p[i + 1] === 2) { st[k] = `rgb(${p[i + 2]},${p[i + 3]},${p[i + 4]})`; i += 4; }
    }
  }
}

function css(st: Sgr): string {
  let fg = st.fg, bg = st.bg;
  if (st.r) [fg, bg] = [bg ?? 'var(--t-bg)', fg ?? 'var(--t-fg)'];
  let s = '';
  if (fg) s += `color:${fg};`;
  if (bg) s += `background:${bg};`;
  if (st.b) s += 'font-weight:700;';
  if (st.d) s += 'opacity:.6;';
  if (st.i) s += 'font-style:italic;';
  if (st.u) s += 'text-decoration:underline;';
  return s;
}

const esc = (t: string) => t.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** HTML for one screen of `cols` columns. */
export function ansiToHtml(screen: string, cols: number): string {
  const st: Sgr = {};
  const out: string[] = [];
  for (const line of screen.split('\n')) {
    let html = '';
    let width = 0;
    let pos = 0;
    const re = /\x1b\[([0-9;]*)m/g;
    const span = (text: string) => {
      if (!text) return;
      width += [...text].length;
      const s = css(st);
      html += s ? `<span style="${s}">${esc(text)}</span>` : esc(text);
    };
    for (let m; (m = re.exec(line)); ) {
      span(line.slice(pos, m.index));
      apply(st, m[1]);
      pos = re.lastIndex;
    }
    span(line.slice(pos));
    if (width < cols) span(' '.repeat(cols - width));
    out.push(html);
  }
  return out.join('\n');
}
