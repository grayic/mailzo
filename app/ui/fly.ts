// Deleted mail and senders left go out as a letter: a small sheet of paper rises out of the
// row in 3D, folds in half, then flies off to the bottom-right corner. Sounds are made on
// the spot.

/** "sound", "quiet" or "off", from the delete_effect setting. */
export type FlyMode = string;

const RISE_MS = 260;
const FOLD_MS = 380;
const FLY_MS = 560;
const LINES = 3;

const reduced = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

let audio: AudioContext | null = null;

/** Noise shaped by a filter: paper rustling, or air rushing past. */
function noise(ctx: AudioContext, at: number, length: number, from: number, to: number, peak: number) {
  const buffer = ctx.createBuffer(1, Math.ceil(ctx.sampleRate * length), ctx.sampleRate);
  const data = buffer.getChannelData(0);
  for (let i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1;
  const source = ctx.createBufferSource();
  source.buffer = buffer;
  const band = ctx.createBiquadFilter();
  band.type = "bandpass";
  band.Q.value = 1.2;
  band.frequency.setValueAtTime(from, at);
  band.frequency.exponentialRampToValueAtTime(to, at + length * 0.85);
  const level = ctx.createGain();
  level.gain.setValueAtTime(0, at);
  level.gain.linearRampToValueAtTime(peak, at + length * 0.3);
  level.gain.exponentialRampToValueAtTime(0.001, at + length);
  source.connect(band).connect(level).connect(ctx.destination);
  source.start(at);
  source.stop(at + length);
}

/** A crisp paper fold, then a soft whoosh as it leaves. */
function playSounds() {
  try {
    audio ??= new AudioContext();
    const ctx = audio;
    if (ctx.state === "suspended") void ctx.resume();
    const now = ctx.currentTime;
    noise(ctx, now + RISE_MS / 1000, 0.16, 5200, 3000, 0.09);
    noise(ctx, now + (RISE_MS + FOLD_MS) / 1000, 0.45, 450, 2400, 0.14);
  } catch {
    // No sound is fine.
  }
}

function div(className: string, parent: HTMLElement, text?: string) {
  const node = document.createElement("div");
  node.className = className;
  if (text) node.textContent = text;
  parent.appendChild(node);
  return node;
}

/** One half of the sheet: a few written lines, and the title on the top half. */
function half(parent: HTMLElement, className: string, title?: string) {
  const side = div(className, parent);
  const face = div("fly-face", side);
  if (title) div("fly-title", face, title);
  for (let i = 0; i < LINES; i++) div(`fly-line${i === LINES - 1 ? " fly-line-short" : ""}`, face);
  div("fly-back", side);
  return side;
}

/**
 * Sends what `node` was (a row about to leave its list) off as a folded letter. Several at
 * once carry a count. Nothing when the setting is off, and only a quick fade when the Mac
 * asks for less motion.
 */
export function flyAway(node: Element | null, mode: FlyMode, count = 1) {
  if (mode === "off" || !(node instanceof HTMLElement)) return;
  const rect = node.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0 || rect.bottom < 0 || rect.top > window.innerHeight) return;
  if (mode === "sound") playSounds();

  const title = node.querySelector(".thread-subject, .news-name")?.textContent?.trim() ?? "";
  const stage = document.createElement("div");
  stage.className = "fly";
  stage.setAttribute("aria-hidden", "true");
  // The sheet starts over the middle of the row.
  stage.style.setProperty("--fly-x", `${rect.left + rect.width / 2}px`);
  stage.style.setProperty("--fly-y", `${rect.top + rect.height / 2}px`);
  const sheet = div("fly-sheet", stage);
  const top = half(sheet, "fly-half fly-half-top", title);
  half(sheet, "fly-half fly-half-bottom");
  if (count > 1) {
    sheet.dataset.stack = "";
    div("fly-count", sheet, String(count));
  }
  document.body.appendChild(stage);
  const end = () => stage.remove();

  if (reduced() || typeof sheet.animate !== "function") {
    sheet.animate?.([{ opacity: 1 }, { opacity: 0 }], { duration: 200, fill: "forwards" });
    setTimeout(end, 220);
    return;
  }

  // 1. It rises out of the row, standing up toward you.
  sheet.animate(
    [
      { transform: "translate(-50%, -50%) translateY(10px) rotateX(70deg) scale(0.5)", opacity: 0 },
      { transform: "translate(-50%, -50%) translateY(-28px) rotateX(12deg) scale(1)", opacity: 1 },
    ],
    { duration: RISE_MS, easing: "cubic-bezier(0.2, 0.9, 0.3, 1.15)", fill: "forwards" },
  );

  // 2. The top half folds down over the bottom.
  top.animate([{ transform: "rotateX(0deg)" }, { transform: "rotateX(-178deg)" }], {
    duration: FOLD_MS,
    delay: RISE_MS,
    easing: "cubic-bezier(0.6, 0, 0.3, 1)",
    fill: "forwards",
  });

  // 3. Folded, it flies off into the bottom-right corner, easing in and out.
  const dx = window.innerWidth - 56 - (rect.left + rect.width / 2);
  const dy = window.innerHeight - 48 - (rect.top + rect.height / 2);
  sheet
    .animate(
      [
        { transform: "translate(-50%, -50%) translateY(-28px) rotateX(12deg) scale(1)", opacity: 1 },
        {
          transform: `translate(-50%, -50%) translate(${dx * 0.45}px, ${dy * 0.3 - 40}px) rotateX(20deg) rotateZ(-8deg) scale(0.8)`,
          opacity: 1,
          offset: 0.45,
        },
        {
          transform: `translate(-50%, -50%) translate(${dx}px, ${dy}px) rotateX(30deg) rotateZ(14deg) scale(0.22)`,
          opacity: 0,
        },
      ],
      { duration: FLY_MS, delay: RISE_MS + FOLD_MS, easing: "cubic-bezier(0.65, 0, 0.35, 1)", fill: "forwards" },
    )
    .finished.then(end, end);
  setTimeout(end, RISE_MS + FOLD_MS + FLY_MS + 300);
}
