/**
 * Email is untrusted. This builds the document shown in the email frame:
 * - a content policy blocks all network requests, so tracking pixels and remote
 *   images never load, unless this email's images are to be shown, which is the
 *   default and can be changed in Settings
 * - anything that can act on its own is removed
 * - every link opens outside the app, and only web and mail links stay clickable
 *
 * DOMParser builds an inert document: nothing in it loads or runs here.
 */
import { cleanLink } from "./cleanLink";
/**
 * When the app fetches pictures itself, only its own route may load them, so a picture
 * the rewriting missed can't reach the sender's server directly. Without that route
 * (in a browser), pictures load from the web as usual.
 */
const imageSources = (images: boolean) =>
  !images ? "" : imageRoute ? " inboximage: http://inboximage.localhost" : " https: http:";
const policy = (images: boolean) =>
  `default-src 'none'; img-src data:${imageSources(images)}; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'`;

/** The usual ways mail apps mark the earlier message in a reply. */
const QUOTED = ".gmail_signature, [data-smartmail='gmail_signature'], .moz-signature, #Signature, .gmail_quote, blockquote[type='cite'], .yahoo_quoted, .moz-cite-prefix, #appendonsend ~ *, #divRplyFwdMsg, #divRplyFwdMsg ~ *";

const BASE_STYLE = `
  :root { color-scheme: light !important; }
  html { overflow-y: hidden; }
  html, body { margin: 0; }
  /* On the body alone, so an email's own colour can fill the whole frame. */
  body { background: #fff; color: #18181b; box-sizing: border-box; }
  /* Many newsletters ask for a full-height page. The frame fits itself to the email's height,
     so that request would leave it at its starting size with the rest cut off. */
  html, body { height: auto !important; min-height: 0 !important; }
  body { padding: 14px 16px; font: 15px/1.55 -apple-system, BlinkMacSystemFont, sans-serif; overflow-wrap: break-word; }
  img { max-width: 100%; height: auto; }
  table { max-width: 100%; }
  pre { white-space: pre-wrap; }
  /* A question for the reader: underlined, so it never looks like selected text. */
  mark.key-fact { background: transparent; color: inherit; text-decoration: underline 2px #e0a800; text-underline-offset: 3px; text-decoration-skip-ink: none; }
`;

/**
 * A plain email (words, links, a signature) shown in the app's dark colours. Its own grey and
 * black text becomes the app's light text, so it reads like the rest of the app. Only used when
 * `canGoDark` finds nothing the sender coloured on purpose. Real values: the frame can't see tokens.
 */
const DARK_STYLE = `
  :root { color-scheme: dark !important; }
  body { background: transparent !important; color: #eeeef0 !important; }
  body *:not(img) { color: inherit !important; background-color: transparent !important; border-color: rgb(255 255 255 / 15%) !important; }
  a, a *:not(img) { color: #a3b8ff !important; }
  blockquote { border-left-color: rgb(255 255 255 / 15%) !important; }
  hr { border-color: rgb(255 255 255 / 15%) !important; }
  /* Logos with see-through parts are drawn for white, so they keep a little white behind them. */
  img:not([style*="background"]) { background-color: #fff; border-radius: 4px; }
  mark.key-fact { background: transparent !important; text-decoration-color: #f2c94c; }
`;

const REMOVE = "script, iframe, frame, frameset, object, embed, applet, meta, base, link, form";
const SAFE_LINK = /^(https?:|mailto:)/i;

const READING_SCALES: Record<string, number> = { small: 0.92, normal: 1, large: 1.1, xlarge: 1.22 };
let readingScale = 1;

/** The chosen text size. This frame can't see the app's tokens, so it's a real value.
 * Zoom rather than a font size, because most emails set their own sizes. */
export function setReadingSize(size: string) {
  readingScale = READING_SCALES[size] ?? 1;
}

const parse = (html: string) => markQuoteIntros(new DOMParser().parseFromString(html, "text/html"));

/** The line a reply puts above the earlier message, as in "On Nov 7, Ana wrote:". */
export const QUOTE_INTRO = /^On .{5,200} wrote:\s*$/;

/** Some apps quote without the usual mark: the intro line, then the earlier message. */
function markQuoteIntros(doc: Document): Document {
  doc.querySelectorAll("blockquote").forEach((quote) => {
    const intro = quote.previousElementSibling;
    if (intro && QUOTE_INTRO.test((intro.textContent ?? "").trim())) {
      intro.classList.add("gmail_quote");
      quote.classList.add("gmail_quote");
    }
  });
  return doc;
}

/** Services whose job includes reporting that an email was opened. Pictures from these
 * addresses never load, even when the person chooses to see an email's pictures. */
const TRACKERS: [string, string][] = [
  ["list-manage.com", "Mailchimp"],
  ["mailchimp.com", "Mailchimp"],
  ["sendgrid.net", "SendGrid"],
  ["sendgrid.com", "SendGrid"],
  ["hubspotemail.net", "HubSpot"],
  ["hs-analytics.net", "HubSpot"],
  ["hsforms.com", "HubSpot"],
  ["mailgun.org", "Mailgun"],
  ["mailgun.com", "Mailgun"],
  ["mandrillapp.com", "Mandrill"],
  ["mcsv.net", "Mailchimp"],
  ["rs6.net", "Constant Contact"],
  ["constantcontact.com", "Constant Contact"],
  ["klaviyo.com", "Klaviyo"],
  ["klclick.com", "Klaviyo"],
  ["trk.klclick1.com", "Klaviyo"],
  ["customer.io", "Customer.io"],
  ["customeriomail.com", "Customer.io"],
  ["intercom-mail.com", "Intercom"],
  ["intercom.io", "Intercom"],
  ["convertkit-mail.com", "ConvertKit"],
  ["ck.page", "ConvertKit"],
  ["activehosted.com", "ActiveCampaign"],
  ["activecampaign.com", "ActiveCampaign"],
  ["mailerlite.com", "MailerLite"],
  ["mlsend.com", "MailerLite"],
  ["campaign-archive.com", "Mailchimp"],
  ["createsend.com", "Campaign Monitor"],
  ["cmail19.com", "Campaign Monitor"],
  ["cmail20.com", "Campaign Monitor"],
  ["sparkpostmail.com", "SparkPost"],
  ["postmarkapp.com", "Postmark"],
  ["pstmrk.it", "Postmark"],
  ["amazonses.com", "Amazon SES"],
  ["awstrack.me", "Amazon SES"],
  ["exct.net", "Salesforce"],
  ["exacttarget.com", "Salesforce"],
  ["pardot.com", "Salesforce"],
  ["salesforce.com", "Salesforce"],
  ["marketo.com", "Marketo"],
  ["mktoresp.com", "Marketo"],
  ["eloqua.com", "Oracle Eloqua"],
  ["bluecore.com", "Bluecore"],
  ["iterable.com", "Iterable"],
  ["braze.com", "Braze"],
  ["appboy.com", "Braze"],
  ["mixmax.com", "Mixmax"],
  ["mailtrack.io", "Mailtrack"],
  ["streak.com", "Streak"],
  ["yesware.com", "Yesware"],
  ["superhuman.com", "Superhuman"],
  ["getnotify.com", "Notify"],
  ["boomeranggmail.com", "Boomerang"],
  ["sendinblue.com", "Brevo"],
  ["brevo.com", "Brevo"],
  ["omnisend.com", "Omnisend"],
  ["drip.com", "Drip"],
  ["getdrip.com", "Drip"],
  ["emltrk.com", "Litmus"],
  ["litmus.com", "Litmus"],
  ["doubleclick.net", "Google"],
  ["google-analytics.com", "Google"],
  ["facebook.com", "Facebook"],
  ["linkedin.com/emimp", "LinkedIn"],
];

const TINY = (img: Element) => {
  const size = (name: string) => Number.parseInt(img.getAttribute(name) ?? "", 10);
  const style = (img.getAttribute("style") ?? "").toLowerCase();
  return (
    (size("width") <= 2 && size("height") <= 2) ||
    /(^|;)\s*(width|height)\s*:\s*[01]px/.test(style) ||
    /display\s*:\s*none/.test(style)
  );
};

/** The tracking company an address belongs to, and that address without its start. */
function trackingCompany(address: string): { company: string | null; url: string } | null {
  const src = address.trim();
  if (!/^(https?:)?\/\//i.test(src)) return null;
  const url = src.replace(/^(https?:)?\/\//i, "").toLowerCase();
  const host = url.split("/")[0];
  for (const [mark, company] of TRACKERS) {
    if (mark.includes("/") ? url.startsWith(mark) || url.includes(`.${mark}`) : host === mark || host.endsWith(`.${mark}`)) {
      return { company, url };
    }
  }
  return { company: null, url };
}

/** Mail services also host real pictures, so an address alone only counts when it says so. */
const TRACKING_ADDRESS = /(open|track|pixel|beacon|wf\/open)/;
const isTrackingAddress = (address: string) => {
  const found = trackingCompany(address);
  return !!found?.company && TRACKING_ADDRESS.test(found.url);
};

/** Who's behind this picture, if it's there to report on the reader. */
function trackerBehind(img: Element): string | null {
  const found = trackingCompany(img.getAttribute("src") ?? "");
  if (!found) return null;
  if (found.company && (TINY(img) || TRACKING_ADDRESS.test(found.url))) return found.company;
  return TINY(img) ? "an unknown company" : null;
}

/** Pictures can also load from a list of sizes, a background or a stylesheet. */
function removeOtherTrackers(doc: Document) {
  const cssUrl = /url\(\s*['"]?([^'")]+)['"]?\s*\)/gi;
  const cleanCss = (css: string) =>
    css.replace(cssUrl, (whole, address: string) => (isTrackingAddress(address) ? "none" : whole));
  doc.querySelectorAll("img[srcset], source[srcset]").forEach((el) => {
    const kept = (el.getAttribute("srcset") ?? "")
      .split(",")
      .filter((entry) => !isTrackingAddress(entry.trim().split(/\s+/)[0] ?? ""));
    if (kept.length > 0) el.setAttribute("srcset", kept.join(","));
    else el.removeAttribute("srcset");
  });
  doc.querySelectorAll("source[src]").forEach((el) => {
    if (isTrackingAddress(el.getAttribute("src") ?? "")) el.removeAttribute("src");
  });
  doc.querySelectorAll("[background]").forEach((el) => {
    if (isTrackingAddress(el.getAttribute("background") ?? "")) el.removeAttribute("background");
  });
  doc.querySelectorAll("[style]").forEach((el) => {
    el.setAttribute("style", cleanCss(el.getAttribute("style") ?? ""));
  });
  doc.querySelectorAll("style").forEach((el) => {
    el.textContent = cleanCss(el.textContent ?? "");
  });
}

/** The companies trying to tell the sender this email was opened. */
/**
 * Two things older emails say in attributes, which the styles here would otherwise overrule: the
 * page's colours, and a picture's shape. `height: auto` alone turns a thin spacer into a tall square.
 */
function keepOldFashionedSizes(doc: Document) {
  // Dark-mode rules assume the email's own dark page. Here it always sits on white paper.
  doc.querySelectorAll("style").forEach((style) => {
    const text = style.textContent ?? "";
    if (/prefers-color-scheme/i.test(text)) {
      style.textContent = text.replace(/prefers-color-scheme\s*:\s*dark/gi, "prefers-color-scheme: never");
    }
  });
  const body = doc.body;
  const fill = body?.getAttribute("bgcolor");
  if (fill) body.style.backgroundColor = fill;
  const ink = body?.getAttribute("text");
  if (ink) body.style.color = ink;
  doc.querySelectorAll("img[width][height]").forEach((el) => {
    const img = el as HTMLImageElement;
    const wide = Number(img.getAttribute("width"));
    const tall = Number(img.getAttribute("height"));
    if (wide > 0 && tall > 0 && !img.style.height) img.style.aspectRatio = `${wide} / ${tall}`;
  });
}

let imageRoute: ((address: string) => string) | null = null;

/** Sends an email's pictures through the app, which can fetch ones browsers refuse to show. */
export function routeImagesThrough(route: (address: string) => string) {
  imageRoute = route;
}

const CSS_PICTURE = /url\(\s*(['"]?)(https?:[^'")]+)\1\s*\)/gi;
const REMOTE = /^https?:/i;

function reroutePictures(doc: Document, route: (address: string) => string) {
  doc.querySelectorAll("img[srcset], source[srcset]").forEach((el) => el.removeAttribute("srcset"));
  doc.querySelectorAll("img[src]").forEach((img) => {
    const address = (img.getAttribute("src") ?? "").trim();
    if (REMOTE.test(address)) img.setAttribute("src", route(address));
  });
  doc.querySelectorAll("[background]").forEach((el) => {
    const address = (el.getAttribute("background") ?? "").trim();
    if (REMOTE.test(address)) el.setAttribute("background", route(address));
  });
  const inCss = (text: string) => text.replace(CSS_PICTURE, (_, __, address: string) => `url("${route(address)}")`);
  doc.querySelectorAll("[style*='url(']").forEach((el) => {
    el.setAttribute("style", inCss(el.getAttribute("style") ?? ""));
  });
  doc.querySelectorAll("style").forEach((style) => {
    style.textContent = inCss(style.textContent ?? "");
  });
}

/** Amounts, the dates things are needed by, and questions: what a quick reader is looking for. */
const KEY_FACT =
  /[$€£]\s?\d[\d,]*(?:\.\d+)?|\b(?:by|before|until|due)\s+(?:today|tomorrow|tonight|(?:mon|tues|wednes|thurs|fri|satur|sun)day|next week|end of (?:the )?(?:day|week|month)|(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+\d{1,2})\b|[^.!?\n]{6,160}\?/gi;
const MOST_KEY_FACTS = 12;

function markKeyFacts(doc: Document) {
  const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n.parentElement?.closest(`style, script, title, textarea, a, ${QUOTED}`)) continue;
    nodes.push(n as Text);
  }
  let marked = 0;
  for (const node of nodes) {
    if (marked >= MOST_KEY_FACTS) return;
    const text = node.nodeValue ?? "";
    const hits = [...text.matchAll(KEY_FACT)];
    if (hits.length === 0) continue;
    const out = doc.createDocumentFragment();
    let at = 0;
    for (const hit of hits) {
      const start = hit.index ?? 0;
      out.append(text.slice(at, start));
      const mark = doc.createElement("mark");
      mark.className = "key-fact";
      mark.title = hit[0].trim().endsWith("?") ? "A question for you" : "Worth noticing";
      mark.textContent = hit[0];
      out.append(mark);
      at = start + hit[0].length;
      marked += 1;
    }
    out.append(text.slice(at));
    node.replaceWith(out);
  }
}

/** How many links in the email carry tracking codes, which come off before it shows. */
export function trackedLinks(html: string): number {
  let count = 0;
  parse(html)
    .querySelectorAll("a[href], area[href]")
    .forEach((link) => {
      if (cleanLink((link.getAttribute("href") ?? "").trim()).removed > 0) count += 1;
    });
  return count;
}

export function findTrackers(html: string): string[] {
  const found = new Set<string>();
  parse(html)
    .querySelectorAll("img")
    .forEach((img) => {
      const who = trackerBehind(img);
      if (who) found.add(who);
    });
  return [...found];
}

/** Wraps the wanted words in a mark, without touching anything but the text. */
function highlightIn(doc: Document, words: string) {
  const wanted = words.trim().toLowerCase();
  if (!wanted) return;
  const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
  const hits: Text[] = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    // Marks inside these would show up as text, or break them.
    if (n.parentElement?.closest("style, script, title, textarea")) continue;
    if ((n.nodeValue ?? "").toLowerCase().includes(wanted)) hits.push(n as Text);
  }
  for (const node of hits) {
    const text = node.nodeValue ?? "";
    const lower = text.toLowerCase();
    const out = doc.createDocumentFragment();
    let at = 0;
    for (let i = lower.indexOf(wanted); i >= 0; i = lower.indexOf(wanted, at)) {
      out.append(text.slice(at, i));
      const mark = doc.createElement("mark");
      mark.textContent = text.slice(i, i + wanted.length);
      out.append(mark);
      at = i + wanted.length;
    }
    out.append(text.slice(at));
    node.replaceWith(out);
  }
}

export function hasRemoteImages(html: string): boolean {
  return [...parse(html).querySelectorAll("img")].some((img) =>
    /^(https?:)?\/\//i.test(img.getAttribute("src") ?? ""),
  );
}

export function hasQuotedPart(html: string): boolean {
  const doc = parse(html);
  const quoted = doc.querySelectorAll(QUOTED);
  if (quoted.length === 0) return false;
  // Sometimes everything is inside the quote, such as a plain forward. Folding would leave nothing.
  quoted.forEach((el) => el.remove());
  doc.querySelectorAll("style, script, title").forEach((el) => el.remove());
  return (doc.body?.textContent ?? "").trim().length >= MIN_WORDS_AFTER_FOLDING || !!doc.querySelector("img");
}

const MIN_WORDS_AFTER_FOLDING = 10;
const FOLD_RULE = `${QUOTED} { display: none !important; }`;
const SMALLEST_FIT = 0.5;
const MOST_ELEMENTS_CHECKED = 4000;

/** A sentence told never to wrap is better wrapped than making the whole email tiny. */
function unwrapLongLines(page: Document) {
  const view = page.defaultView;
  const all = page.body.getElementsByTagName("*");
  for (let i = 0; i < Math.min(all.length, MOST_ELEMENTS_CHECKED); i += 1) {
    const el = all[i] as HTMLElement;
    if ((el.textContent?.length ?? 0) < 40 || el.tagName === "TABLE") continue;
    const rule = view?.getComputedStyle(el).whiteSpace;
    if (rule === "nowrap" || rule === "pre") el.style.whiteSpace = rule === "pre" ? "pre-wrap" : "normal";
  }
}

/**
 * Fits an email to its frame and says how tall it is. A frame shows an email's top-left at full
 * size, so one wider than the frame is shrunk to fit, as Apple Mail does. The height is read with
 * the frame nearly closed, which counts margins and floats, and stops emails that size themselves
 * to the window from growing with it.
 */
export function fitEmail(frame: HTMLIFrameElement): number {
  const page = frame.contentDocument;
  if (!page?.body) return 0;
  const root = page.documentElement;
  // Folding a reply away can leave nothing, when all it held was a signature or a forward.
  const nothingLeft = page.body.innerText.trim().length < MIN_WORDS_AFTER_FOLDING && !page.querySelector("img");
  if (nothingLeft) {
    page.querySelectorAll("style").forEach((style) => {
      if (style.textContent?.includes(FOLD_RULE)) style.textContent = style.textContent.replace(FOLD_RULE, "");
    });
  }
  const kept = frame.style.height;
  frame.style.height = "1px";
  root.style.setProperty("zoom", String(readingScale));
  if (root.scrollWidth > root.clientWidth + 1) unwrapLongLines(page);
  for (let i = 0, zoom = readingScale; i < 4 && root.scrollWidth > root.clientWidth + 1; i += 1) {
    zoom = Math.max(zoom * (root.clientWidth / root.scrollWidth), readingScale * SMALLEST_FIT);
    root.style.setProperty("zoom", String(zoom));
  }
  const height = root.scrollHeight;
  frame.style.height = kept;
  return height;
}

/** A colour written in an email, read as red, green and blue, or null if it can't be read. */
function readColour(value: string): [number, number, number] | null {
  const v = value.trim().toLowerCase().replace(/\s*!important$/, "");
  const named: Record<string, [number, number, number]> = {
    black: [0, 0, 0],
    white: [255, 255, 255],
    gray: [128, 128, 128],
    grey: [128, 128, 128],
    dimgray: [105, 105, 105],
    dimgrey: [105, 105, 105],
    darkgray: [169, 169, 169],
    darkgrey: [169, 169, 169],
  };
  if (named[v]) return named[v];
  const hex = v.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/);
  if (hex) {
    const h = hex[1].length === 3 ? [...hex[1]].map((c) => c + c).join("") : hex[1];
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as [number, number, number];
  }
  const rgb = v.match(/^rgba?\(\s*(\d+)[\s,]+(\d+)[\s,]+(\d+)/);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  return null;
}

/** Grey or black: text the sender didn't really colour. */
const plainInk = (value: string) => {
  const v = value.trim().toLowerCase();
  if (["inherit", "initial", "currentcolor", "unset", "windowtext", ""].includes(v)) return true;
  const c = readColour(v);
  if (!c) return false;
  // Grey up to a mid grey, or any colour dark enough to read as black, such as navy.
  return (Math.max(...c) - Math.min(...c) <= 24 && Math.max(...c) <= 140) || Math.max(...c) <= 100;
};

/** No fill at all, or white: the page the sender expected anyway. */
const noFill = (value: string) => {
  const v = value.trim().toLowerCase().replace(/\s*!important$/, "");
  if (["", "none", "transparent", "inherit", "initial", "unset", "white", "#fff", "#ffffff"].includes(v)) return true;
  const c = readColour(v);
  return !!c && Math.min(...c) >= 250;
};

/**
 * Whether an email is plain enough to show in dark colours: no backgrounds or pictures behind
 * text, no wide newsletter layout, and only grey or black text. Anything designed stays on
 * white, where it was meant to be read.
 */
export function canGoDark(doc: Document): boolean {
  for (const el of doc.querySelectorAll("[bgcolor], [background]")) {
    if (el.hasAttribute("background") || !noFill(el.getAttribute("bgcolor") ?? "")) return false;
  }
  for (const el of doc.querySelectorAll("font[color]")) {
    if (!el.closest("a") && !plainInk(el.getAttribute("color") ?? "")) return false;
  }
  const body = doc.body;
  if (body && (body.getAttribute("text") && !plainInk(body.getAttribute("text") ?? ""))) return false;
  for (const el of doc.querySelectorAll("table[width], td[width]")) {
    const width = Number.parseInt(el.getAttribute("width") ?? "", 10);
    if (width >= 480) return false;
  }
  for (const el of doc.querySelectorAll("[style]")) {
    const style = (el as HTMLElement).style;
    // A fill behind a picture, such as a logo's circle, stays with the picture.
    if (el.tagName !== "IMG") {
      if (style.backgroundImage && style.backgroundImage !== "none") return false;
      if (!noFill(style.backgroundColor)) return false;
      if (style.background && !noFill(style.background)) return false;
    }
    // Links get the app's own link colour, whatever the email says.
    if (style.color && !el.closest("a") && !plainInk(style.color)) return false;
  }
  for (const style of doc.querySelectorAll("style")) {
    const text = style.textContent ?? "";
    for (const m of text.matchAll(/background(?:-color|-image)?\s*:\s*([^;}]+)/gi)) {
      if (!noFill(m[1])) return false;
    }
    for (const m of text.matchAll(/(?:^|[;{\s])color\s*:\s*([^;}]+)/gi)) {
      if (!plainInk(m[1])) return false;
    }
  }
  return true;
}

export function prepareEmail(
  html: string,
  options: { showImages?: boolean; hideQuoted?: boolean; highlight?: string; keyFacts?: boolean; dark?: boolean } = {},
): { doc: string; dark: boolean } {
  const doc = parse(html);

  doc.querySelectorAll(REMOVE).forEach((el) => el.remove());
  // Showing an email's pictures doesn't mean telling the sender it was opened.
  doc.querySelectorAll("img").forEach((img) => {
    if (trackerBehind(img)) img.remove();
  });
  removeOtherTrackers(doc);
  // Read before anything here changes the email, so only the sender's own colours count.
  const dark = !!options.dark && canGoDark(doc);
  if (options.highlight) highlightIn(doc, options.highlight);
  keepOldFashionedSizes(doc);
  if (options.showImages && imageRoute) reroutePictures(doc, imageRoute);
  if (options.keyFacts) markKeyFacts(doc);

  doc.querySelectorAll("*").forEach((el) => {
    for (const attr of [...el.attributes]) {
      if (attr.name.toLowerCase().startsWith("on")) el.removeAttribute(attr.name);
    }
  });

  doc.querySelectorAll("a, area").forEach((link) => {
    const href = (link.getAttribute("href") ?? "").trim();
    if (SAFE_LINK.test(href)) {
      // The website still opens, without learning who clicked from which email.
      link.setAttribute("href", cleanLink(href).href);
      link.setAttribute("target", "_blank");
      link.setAttribute("rel", "noopener noreferrer");
    } else {
      link.removeAttribute("href");
      link.removeAttribute("target");
    }
  });

  // These go first, so the policy covers everything after it.
  const style = doc.createElement("style");
  const sized = `${BASE_STYLE}\nhtml { zoom: ${readingScale}; }${dark ? DARK_STYLE : ""}`;
  style.textContent = options.hideQuoted
    ? `${sized}\n${FOLD_RULE}`
    : sized;
  doc.head.prepend(style);
  const rules = doc.createElement("meta");
  rules.setAttribute("http-equiv", "Content-Security-Policy");
  rules.setAttribute("content", policy(options.showImages ?? false));
  doc.head.prepend(rules);
  const charset = doc.createElement("meta");
  charset.setAttribute("charset", "utf-8");
  doc.head.prepend(charset);

  return { doc: `<!doctype html>${doc.documentElement.outerHTML}`, dark };
}
