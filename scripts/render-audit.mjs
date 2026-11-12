// Displays real emails the way the app does, and beside each one the email untouched, then
// reports what breaks. Everything stays on this machine: every network request is refused, so
// no sender can tell an email was opened, and the report holds numbers and sender domains only.
//
//   PLAYWRIGHT=/path/to/node_modules/playwright node scripts/render-audit.mjs \
//     --db mail.db --out report.jsonl [--limit 500] [--per-sender 3] [--engine webkit] [--ids 1,2,3]

import { createRequire, stripTypeScriptTypes } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { appendFileSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const arg = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at > 0 ? process.argv[at + 1] : fallback;
};
const db = new DatabaseSync(arg("db"), { readOnly: true });
const out = arg("out", "render-report.jsonl");
const limit = Number(arg("limit", "100000"));
const perSender = Number(arg("per-sender", "3"));
const engine = arg("engine", "webkit");
const shots = arg("shots", "");
const ids = arg("ids", "");
const WIDTHS = [640, 400];
const HOST = "http://render-audit.test/";

const require = createRequire(import.meta.url);
const playwright = require(process.env.PLAYWRIGHT);

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "../app/ui/prepareEmail.ts"), "utf8").replace(/^export /gm, "");
const prepareCode =
  stripTypeScriptTypes(source) +
  "\nwindow.prepareEmail = prepareEmail; window.hasQuotedPart = hasQuotedPart; window.fitEmail = fitEmail;";

const rows = ids
  ? db
      .prepare(
        `SELECT m.id, m.from_addr, b.html FROM messages m JOIN bodies b ON b.message_id = m.id
         WHERE m.id IN (${ids.split(",").map(Number).join(",")})`,
      )
      .all()
  : db
      .prepare(
        `SELECT id, from_addr, html FROM (
           SELECT m.id, m.from_addr, b.html, ROW_NUMBER() OVER (
             PARTITION BY lower(m.from_addr) ORDER BY m.date DESC) AS n
           FROM messages m JOIN bodies b ON b.message_id = m.id
           WHERE b.html IS NOT NULL AND length(b.html) > 0)
         WHERE n <= ? ORDER BY id DESC LIMIT ?`,
      )
      .all(perSender, limit);

// Runs inside the browser.
async function auditOne(html, widths) {
  const show = async (doc, width) => {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-same-origin");
    frame.style.cssText = `position:absolute;left:0;top:0;width:${width}px;height:120px;border:0`;
    frame.srcdoc = doc;
    document.body.append(frame);
    await new Promise((done) => {
      frame.onload = done;
      setTimeout(done, 2500);
    });
    return frame;
  };
  const visibleText = (d) => (d.body?.innerText ?? "").replace(/\s+/g, " ").trim();

  const contrast = (a, b) => {
    const lum = ([r, g, bl]) => {
      const f = (v) => {
        v /= 255;
        return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(bl);
    };
    const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
    return (x + 0.05) / (y + 0.05);
  };
  const rgba = (text) => {
    const m = text.match(/[\d.]+/g)?.map(Number) ?? [0, 0, 0, 0];
    return { rgb: m.slice(0, 3), a: m.length > 3 ? m[3] : 1 };
  };

  const inspect = (frame, width) => {
    const d = frame.contentDocument;
    const win = frame.contentWindow;
    const report = { vertical: 0, faint: 0, tinyColumn: 0, cutRight: 0, examples: [] };
    if (!d?.body) return { ...report, height: 0, scrollHeight: 0, scrollWidth: 0, text: 0 };
    let lowest = 0;
    let furthest = 0;
    const walker = d.createTreeWalker(d.body, NodeFilter.SHOW_TEXT);
    let seen = 0;
    for (let n = walker.nextNode(); n && seen < 1500; n = walker.nextNode()) {
      const words = (n.nodeValue ?? "").trim();
      const el = n.parentElement;
      if (!words || !el || el.closest("style, script, title")) continue;
      seen += 1;
      const style = win.getComputedStyle(el);
      if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) continue;
      const range = d.createRange();
      range.selectNodeContents(n);
      const rects = [...range.getClientRects()].filter((r) => r.width > 0 && r.height > 0);
      if (!rects.length) continue;
      const box = range.getBoundingClientRect();
      if (parseFloat(style.fontSize) <= 2) continue;
      // Preheaders hide themselves with tiny boxes, and aren't meant to be seen.
      if (el.closest("[style*='max-height:0'],[style*='max-height: 0'],[style*='mso-hide']")) continue;
      lowest = Math.max(lowest, box.bottom);
      furthest = Math.max(furthest, box.right);
      if (box.right > width + 2 && box.left < width) report.cutRight += words.length;
      // One or two letters to a line: a column squeezed to nothing.
      const lines = new Set(rects.map((r) => Math.round(r.top))).size;
      if (words.length >= 4 && lines >= 3 && lines >= words.replace(/\s/g, "").length * 0.5) {
        report.vertical += 1;
        if (report.examples.length < 2) report.examples.push(`vertical:${Math.round(box.width)}x${Math.round(box.height)}`);
      }
      // Text that can't be seen against what's behind it. A picture behind it can't be judged.
      let bg = null;
      let pictured = false;
      for (let up = el; up; up = up.parentElement) {
        const s = win.getComputedStyle(up);
        if (s.backgroundImage !== "none" || up.getAttribute("background")) pictured = true;
        const c = rgba(s.backgroundColor);
        if (c.a > 0.5) {
          bg = c.rgb;
          break;
        }
      }
      if (!pictured) {
        const fg = rgba(style.color);
        if (fg.a > 0.2 && contrast(fg.rgb, bg ?? [255, 255, 255]) < 1.4) report.faint += words.length;
      }
    }
    return {
      ...report,
      height: d.body.offsetHeight,
      scrollHeight: d.documentElement.scrollHeight,
      scrollWidth: d.documentElement.scrollWidth,
      lowest: Math.round(lowest),
      furthest: Math.round(furthest),
      text: visibleText(d).length,
    };
  };

  const result = { widths: {} };
  for (const width of widths) {
    const oursDoc = window.prepareEmail(html, { showImages: false, hideQuoted: false }).doc;
    const raw = await show(html, width);
    raw.style.height = "1000px";
    const rawSeen = inspect(raw, width);
    raw.remove();
    const ours = await show(oursDoc, width);
    // Exactly what the app does, twice, to catch an email that grows each time it's fitted.
    const firstHeight = window.fitEmail(ours);
    ours.style.height = `${firstHeight}px`;
    const secondHeight = window.fitEmail(ours);
    ours.style.height = `${secondHeight}px`;
    const settled = inspect(ours, width);
    const page = ours.contentDocument;
    settled.height = secondHeight;
    settled.zoomed = Number(page?.documentElement.style.zoom ?? 1) < 1;
    settled.frameScrollHeight = page?.documentElement.scrollHeight ?? 0;
    settled.cutWidth = Math.max(0, (page?.documentElement.scrollWidth ?? 0) - (page?.documentElement.clientWidth ?? 0));
    ours.remove();
    const grew = secondHeight - firstHeight;
    ours.remove();
    result.widths[width] = { raw: rawSeen, ours: settled, grew };
  }
  if (window.hasQuotedPart(html)) {
    const folded = await show(window.prepareEmail(html, { showImages: false, hideQuoted: true }).doc, widths[0]);
    window.fitEmail(folded);
    result.foldedText = visibleText(folded.contentDocument);
    result.foldedText = result.foldedText.length;
    result.foldedPictures = folded.contentDocument.querySelectorAll("img").length;
    folded.remove();
  }
  return result;
}

// For a run in the Mac's own engine: render-audit.swift does the displaying, and this scores it.
const jobPath = arg("webkit-job", "");
if (jobPath) {
  writeFileSync(
    jobPath,
    JSON.stringify({
      prepareCode,
      auditCode: auditOne.toString(),
      widths: WIDTHS,
      rows: rows.map((r) => ({ id: Number(r.id), domain: (r.from_addr ?? "").split("@")[1]?.toLowerCase() ?? "", html: r.html })),
    }),
  );
  console.log("wrote", rows.length);
  process.exit(0);
}
const scorePath = arg("score", "");

const domainOf = (address) => (address ?? "").split("@")[1]?.toLowerCase() ?? "";
function problemsOf(r) {
  const problems = [];
  for (const [width, { raw, ours, grew }] of Object.entries(r.widths)) {
    const w = Number(width);
    if (ours.vertical > raw.vertical) problems.push(`squeezed@${w}`);
    // WebKit gives text positions inside a shrunk email in unshrunk units, so those can't be compared.
    if (ours.frameScrollHeight > ours.height + 2 || (!ours.zoomed && ours.lowest > ours.height + 2)) problems.push(`cut_bottom@${w}`);
    if (grew > 20) problems.push(`keeps_growing@${w}`);
    if (raw.text > 40 && ours.text < raw.text * 0.8) problems.push(`text_lost@${w}`);
    if (raw.text > 40 && ours.height < 40) problems.push(`nearly_empty@${w}`);
    if (ours.cutWidth > 2) problems.push(`cut_right@${w}`);
    if (ours.faint > 20) problems.push(raw.faint > 20 ? `faint_also_raw@${w}` : `faint@${w}`);
    // The untouched email fills its 1000-tall test frame when it asks for a full-height page.
    if (raw.height > 100 && raw.height !== 1000 && ours.height < raw.height * 0.6 && ours.cutWidth === 0 && w === WIDTHS[0] && !ours.zoomed) problems.push(`much_shorter@${w}`);
  }
  if (r.foldedText !== undefined && r.widths[WIDTHS[0]].ours.text > 40 && r.foldedText < 10 && !r.foldedPictures) problems.push("blank_when_folded");
  return problems;
}

if (scorePath) {
  const tally = {};
  const examples = {};
  let count = 0;
  let failed = 0;
  for (const line of readFileSync(scorePath, "utf8").split("\n").filter(Boolean)) {
    const r = JSON.parse(line);
    count += 1;
    if (!r.widths) {
      failed += 1;
      continue;
    }
    for (const p of new Set(problemsOf(r))) {
      tally[p] = (tally[p] ?? 0) + 1;
      (examples[p] ??= []).length < 6 && examples[p].push(r.id);
    }
  }
  console.log("scored", count, "failed", failed);
  console.log(JSON.stringify(tally, null, 1));
  console.log(JSON.stringify(examples));
  process.exit(0);
}

const browser = await playwright[engine].launch(
  {
    ...(engine === "chromium" ? { args: ["--use-mock-keychain", "--password-store=basic"] } : {}),
    ...(process.env.ENGINE_PATH ? { executablePath: process.env.ENGINE_PATH } : {}),
  },
);
const context = await browser.newContext({
  viewport: { width: 1000, height: 1000 },
  colorScheme: arg("scheme", "light"),
});
await context.route("**/*", (route) =>
  route.request().url() === HOST
    ? route.fulfill({ contentType: "text/html", body: "<!doctype html><title>audit</title><body></body>" })
    : route.abort(),
);

const WORKERS = 4;
let next = 0;
const tally = {};
let done = 0;
let failed = 0;
writeFileSync(out, "");
if (shots) mkdirSync(shots, { recursive: true });
async function worker() {
  let page = await context.newPage();
  const fresh = async () => {
    await page.goto(HOST);
    await page.addScriptTag({ content: prepareCode });
  };
  await fresh();
  while (next < rows.length) {
    const row = rows[next++];
    try {
      const r = await Promise.race([
        page.evaluate(([html, widths, code]) => new Function("html", "widths", `return (${code})(html, widths)`)(html, widths), [
          row.html,
          WIDTHS,
          auditOne.toString(),
        ]),
        new Promise((_, no) => setTimeout(() => no(new Error("timeout")), 30000)),
      ]);
      const problems = problemsOf(r);
      for (const p of new Set(problems)) tally[p] = (tally[p] ?? 0) + 1;
      appendFileSync(out, JSON.stringify({ id: row.id, domain: domainOf(row.from_addr), problems, ...r }) + "\n");
    } catch (e) {
      failed += 1;
      appendFileSync(out, JSON.stringify({ id: row.id, domain: domainOf(row.from_addr), problems: ["audit_failed"], why: String(e).slice(0, 120) }) + "\n");
      await page.close().catch(() => {});
      page = await context.newPage();
      await fresh();
    }
    done += 1;
    if (done % 200 === 0) console.log(done, "of", rows.length, JSON.stringify(tally));
  }
}
await Promise.all(Array.from({ length: WORKERS }, worker));
await browser.close();
console.log("checked", done, "failed", failed);
console.log(JSON.stringify(tally, null, 1));
