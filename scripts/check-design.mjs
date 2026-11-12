#!/usr/bin/env node
// Keeps the app consistent: raw colours and sizes belong only in tokens.css, and
// every screen uses the shared tokens, components and icon list. See docs/DESIGN.md.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ui = fileURLToPath(new URL("../app/ui/", import.meta.url));
const problems = [];
const report = (file, line, text, why) => problems.push(`${file}:${line}  ${why}\n    ${text.trim()}`);

const tokens = readFileSync(join(ui, "tokens.css"), "utf8");
const defined = new Set([...tokens.matchAll(/(--[\w-]+)\s*:/g)].map((m) => m[1]));
defined.add("--h"); // Each avatar's hue, set from the person's name.

// In these, a raw size is a mistake. Hairlines (borders and rings) are allowed.
const TOKEN_ONLY = /^\s*(font-size|font-weight|padding(-\w+)?|margin(-\w+)?|gap|border-radius|z-index|line-height|letter-spacing|height|min-height|max-height|width|min-width|max-width|font-family|top|left|right|bottom)\s*:\s*(.+);/;
const HAIRLINE = /^(0 auto|auto|1px|1\.5px|2px|0|100%|auto|none|normal|0\.\d+|1|-?\d+(\.\d+)?%|inherit|calc\(100% .*\)|min-content)$/;

// The chosen dark theme and the Mac's dark mode must be the same colours.
const blocks = tokens.match(/:root:not\(\[data-theme="light"\]\) \{([\s\S]*?)\n  \}\n\}[\s\S]*?:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/);
const squeeze = (t) => t.replace(/\s+/g, " ").trim();
if (!blocks || squeeze(blocks[1]) !== squeeze(blocks[2])) {
  report("tokens.css", 1, "dark theme blocks", "the two dark theme blocks must match");
}

const cssFile = "styles.css";
readFileSync(join(ui, cssFile), "utf8").split("\n").forEach((text, i) => {
  const line = i + 1;
  const bare = text.replace(/url\(.*\)/g, "");
  if (/#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(\d/.test(bare)) report(cssFile, line, text, "raw colour, use a token");
  const m = text.match(TOKEN_ONLY);
  if (m && !m[4].includes("var(--") && !HAIRLINE.test(m[4].replace(/\s*!important/, ""))) {
    report(cssFile, line, text, `raw ${m[1]}, use a token`);
  }
  if (/\b\d+m?s\b/.test(bare) && /(transition|animation)/.test(text) && !/1ms|infinite/.test(text)) {
    report(cssFile, line, text, "raw duration, use --dur-fast or --dur-base");
  }
  for (const v of bare.matchAll(/var\((--[\w-]+)/g)) {
    if (!defined.has(v[1])) report(cssFile, line, text, `${v[1]} isn't a token`);
  }
});

const files = [];
for (const dir of ["", "components"]) {
  for (const f of readdirSync(join(ui, dir), { withFileTypes: true })) {
    if (f.isFile() && /\.tsx?$/.test(f.name)) files.push(join(dir, f.name));
  }
}
for (const file of files) {
  const source = readFileSync(join(ui, file), "utf8");
  if (file !== "icons.ts" && source.includes("@hugeicons/core-free-icons")) {
    report(file, 1, "@hugeicons/core-free-icons", "icons come through icons.ts");
  }
  // Colours and sizes in the rich editor become part of the email itself, and the
  // email frame's own styles must work with no access to the app's tokens.
  const skip = file === "prepareEmail.ts";
  source.split("\n").forEach((text, i) => {
    const line = i + 1;
    if (skip || file === "icons.ts") return;
    const emailContent = file.endsWith("RichEditor.tsx") && /^\s*(\["|")/.test(text);
    if (!emailContent && /#[0-9a-fA-F]{3,8}\b/.test(text.replace(/&#\d+;/g, ""))) {
      report(file, line, text, "raw colour, use a token");
    }
    if (/className="[^"]*(-\[|\b(p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ml|mr|gap|space-[xy])-\d|\b(text|bg|border)-(xs|sm|base|lg|xl|\d?xl|white|black|gray|red|blue))/.test(text)) {
      report(file, line, text, "one-off styling class, use the design system's classes");
    }
    const style = text.match(/style=\{\{(.*)\}\}/);
    if (style && !/var\(--|height \}|"--h"|background: c\b/.test(style[0]) && !/^\s*style=\{\{ height \}\}/.test(text)) {
      report(file, line, text, "one-off inline style");
    }
    for (const v of text.matchAll(/var\((--[\w-]+)/g)) {
      if (!defined.has(v[1])) report(file, line, text, `${v[1]} isn't a token`);
    }
  });
}

if (problems.length) {
  console.error(`Design check found ${problems.length} problem(s):\n`);
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log(`Design check passed: ${defined.size} tokens, ${files.length} files.`);
