// Tracking codes that mail senders add to links, so the website can tell it was this
// person who clicked, in this email. Removing them never changes where a link goes.
const TRACKING = new Set([
  "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "twclid", "ttclid",
  "li_fat_id", "igshid", "mc_cid", "mc_eid", "_hsenc", "_hsmi", "__hssc", "__hstc", "__hsfp",
  "hsctatracking", "mkt_tok", "vero_id", "vero_conv", "oly_anon_id", "oly_enc_id", "_openstat",
  "rb_clickid", "s_cid", "ss_source", "ss_campaign_id", "ck_subscriber_id", "ml_subscriber",
  "ml_subscriber_hash", "_kx", "__s", "wickedid", "irclickid", "epik", "spm",
]);

const isTracking = (name: string) => {
  const key = name.toLowerCase();
  return key.startsWith("utm_") || TRACKING.has(key);
};

/** The same link without its tracking codes, and how many came off. Anything odd is left alone. */
export function cleanLink(href: string): { href: string; removed: number } {
  if (!/^https?:/i.test(href)) return { href, removed: 0 };
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return { href, removed: 0 };
  }
  const names = [...url.searchParams.keys()];
  const gone = names.filter(isTracking);
  if (gone.length === 0) return { href, removed: 0 };
  for (const name of new Set(gone)) url.searchParams.delete(name);
  return { href: url.toString(), removed: gone.length };
}
