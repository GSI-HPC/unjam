// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

// node render.mjs CAP.jsonl OUT_DIR TITLE: turns one shot of capture.py into 1920x1080 PNGs
// (a Nord window with the terminal, a caption below it) and OUT_DIR/frames.txt, an ffmpeg
// concat list that holds each PNG for its time on screen.
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';

const { chromium } = createRequire(import.meta.url)('playwright');

const [capPath, outDir, title] = process.argv.slice(2);
const FG = '#d8dee9', BG = '#2e3440';
const base16 = ['#3b4252', '#bf616a', '#a3be8c', '#ebcb8b', '#81a1c1', '#b48ead', '#88c0d0', '#e5e9f0',
  '#4c566a', '#bf616a', '#a3be8c', '#ebcb8b', '#81a1c1', '#b48ead', '#8fbcbb', '#eceff4'];

function x256(n) {
  if (n < 16) return base16[n];
  if (n < 232) {
    n -= 16; const c = [0, 95, 135, 175, 215, 255];
    return `rgb(${c[Math.floor(n / 36)]},${c[Math.floor(n / 6) % 6]},${c[n % 6]})`;
  }
  const v = 8 + (n - 232) * 10; return `rgb(${v},${v},${v})`;
}

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

// SGR-only ANSI (tmux capture-pane -e) to HTML spans; returns [html, plain text] per line.
function ansiToHtml(ansi) {
  const st = { fg: null, bg: null, b: false, d: false, i: false, u: false, r: false };
  const sgr = (params) => {
    const p = params ? params.split(';').map((x) => (x ? +x : 0)) : [0];
    for (let i = 0; i < p.length; i++) {
      const c = p[i];
      if (c === 0) Object.assign(st, { fg: null, bg: null, b: false, d: false, i: false, u: false, r: false });
      else if (c === 1) st.b = true; else if (c === 2) st.d = true; else if (c === 3) st.i = true;
      else if (c === 4) st.u = true; else if (c === 7) st.r = true;
      else if (c === 22) st.b = st.d = false; else if (c === 23) st.i = false;
      else if (c === 24) st.u = false; else if (c === 27) st.r = false;
      else if (c >= 30 && c <= 37) st.fg = base16[c - 30]; else if (c >= 90 && c <= 97) st.fg = base16[c - 82];
      else if (c >= 40 && c <= 47) st.bg = base16[c - 40]; else if (c >= 100 && c <= 107) st.bg = base16[c - 92];
      else if (c === 39) st.fg = null; else if (c === 49) st.bg = null;
      else if (c === 38 || c === 48) {
        const k = c === 38 ? 'fg' : 'bg';
        if (p[i + 1] === 5) { st[k] = x256(p[i + 2]); i += 2; }
        else if (p[i + 1] === 2) { st[k] = `rgb(${p[i + 2]},${p[i + 3]},${p[i + 4]})`; i += 4; }
      }
    }
  };
  const style = () => {
    let fg = st.fg || FG, bg = st.bg || BG;
    if (st.r) [fg, bg] = [bg, fg];
    let s = `color:${fg};`;
    if (bg !== BG) s += `background:${bg};`;
    if (st.b) s += 'font-weight:700;';
    if (st.d) s += 'opacity:.6;';
    if (st.i) s += 'font-style:italic;';
    if (st.u) s += 'text-decoration:underline;';
    return s;
  };
  return ansi.replace(/\n+$/, '').split('\n').map((line) => {
    let html = '', plain = '', pos = 0;
    for (const m of line.matchAll(/\x1b\[([0-9;]*)m/g)) {
      if (m.index > pos) { const t = line.slice(pos, m.index); html += `<span style="${style()}">${esc(t)}</span>`; plain += t; }
      sgr(m[1]); pos = m.index + m[0].length;
    }
    if (pos < line.length) { const t = line.slice(pos); html += `<span style="${style()}">${esc(t)}</span>`; plain += t; }
    return [html, plain];
  });
}

// "key  ·  text" captions show the key part as a keycap.
function caption(cap) {
  if (!cap) return '';
  const i = cap.indexOf('  ·  ');
  if (i < 0) return `<span class="txt">${esc(cap)}</span>`;
  return `<span class="key">${esc(cap.slice(0, i))}</span><span class="txt">${esc(cap.slice(i + 5))}</span>`;
}

const page_ = `<!doctype html><meta charset="utf-8"><style>
@font-face{font-family:Blocks;src:local('DejaVu Sans Mono');unicode-range:U+2580-259F}
html,body{margin:0;width:1920px;height:1080px;overflow:hidden}
body{background:radial-gradient(ellipse at 50% 35%,#3b4252 0%,#2e3440 45%,#242933 100%);font-family:Inter,sans-serif}
.win{position:absolute;left:50%;top:58px;transform:translateX(-50%);background:${BG};border-radius:14px;
  border:1px solid #4c566a;box-shadow:0 30px 80px rgba(0,0,0,.45),0 0 0 1px rgba(136,192,208,.08);overflow:hidden}
.bar{height:42px;background:#3b4252;display:flex;align-items:center;padding:0 18px;gap:9px;position:relative}
.dot{width:13px;height:13px;border-radius:50%}
.ttl{position:absolute;left:0;right:0;text-align:center;font:500 16px 'JetBrains Mono',monospace;color:#a3adc0}
.term{position:relative;padding:14px 18px}
pre{margin:0;font:20px/26px Blocks,'JetBrains Mono','DejaVu Sans Mono',monospace;color:${FG};white-space:pre}
.hl{position:absolute;left:10px;right:10px;border:2px solid #ebcb8b;border-radius:6px;background:rgba(235,203,139,.10);
  box-shadow:0 0 18px rgba(235,203,139,.45)}
.cap{position:absolute;left:0;right:0;top:948px;text-align:center;font-size:34px;color:#eceff4;display:flex;
  justify-content:center;align-items:center;gap:22px}
.key{font:600 28px 'JetBrains Mono',monospace;color:#2e3440;background:#88c0d0;border-radius:10px;padding:6px 16px}
.txt{font-weight:500}
</style><div class="win"><div class="bar"><span class="dot" style="background:#bf616a"></span>
<span class="dot" style="background:#ebcb8b"></span><span class="dot" style="background:#a3be8c"></span>
<span class="ttl" id="ttl"></span></div><div class="term"><pre id="pre"></pre><div id="hls"></div></div></div>
<div class="cap" id="cap"></div>`;

const lines = fs.readFileSync(capPath, 'utf8').trim().split('\n').map((l) => JSON.parse(l));
fs.mkdirSync(outDir, { recursive: true });
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
await page.setContent(page_);
await page.evaluate(() => document.fonts.ready);
const list = [];
let prevKey = null, n = 0;
for (let k = 0; k < lines.length; k++) {
  const { ansi, cap, hl, t } = lines[k];
  const key = JSON.stringify([ansi, cap, hl]);
  const dur = (k + 1 < lines.length ? lines[k + 1].t : t + 1 / 15) - t;
  if (key === prevKey) { list[list.length - 1].dur += dur; continue; }
  prevKey = key;
  const rows = ansiToHtml(ansi);
  const re = hl ? new RegExp(hl) : null;
  const marks = re ? rows.map(([, p], i) => (re.test(p) ? i : -1)).filter((i) => i >= 0) : [];
  const file = path.join(outDir, `f${String(n++).padStart(5, '0')}.png`);
  await page.evaluate(({ html, marks, cap, title }) => {
    document.getElementById('pre').innerHTML = html;
    document.getElementById('ttl').textContent = title;
    document.getElementById('cap').innerHTML = cap;
    document.getElementById('hls').innerHTML = marks
      .map((i) => `<div class="hl" style="top:${14 + i * 26 - 2}px;height:${26 + 0}px"></div>`).join('');
  }, { html: rows.map(([h]) => h).join('\n'), marks, cap: caption(cap), title });
  await page.screenshot({ path: file });
  list.push({ file: path.basename(file), dur });
}
await browser.close();
// The concat demuxer needs the last file twice to honour its duration.
const txt = list.map((e) => `file '${e.file}'\nduration ${e.dur.toFixed(3)}`).join('\n') + `\nfile '${list.at(-1).file}'\n`;
fs.writeFileSync(path.join(outDir, 'frames.txt'), txt);
console.log(`${outDir}: ${list.length} frames`);
