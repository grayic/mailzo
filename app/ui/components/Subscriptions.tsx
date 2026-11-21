import { useEffect, useMemo, useState, type RefObject } from "react";
import { keys } from "../keys";
import {
  ApiError,
  api,
  type Subscription,
  type SubscriptionEdit,
  type SubscriptionPeriod,
  type Subscriptions as Found,
} from "../api";
import { language, tx } from "../i18n";
import {
  Add01Icon,
  AlertCircleIcon,
  ArrowRight01Icon,
  Calendar03Icon,
  Delete02Icon,
  DollarCircleIcon,
  Edit02Icon,
  LinkSquare02Icon,
  Mail01Icon,
  Menu01Icon,
  MoreHorizontalIcon,
  RepeatIcon,
  Search01Icon,
  Tick02Icon,
  ViewIcon,
  ViewOffSlashIcon,
  Cancel01Icon,
} from "../icons";
import type { Account } from "../types";
import { AccountDot, Avatar, Dialog, Icon, IconButton, Menu, MenuItem, Problem, Spinner, accountHue } from "./ui";
import { Choice } from "./settings/controls";
import { DateField } from "./DateField";

const DAY = 86_400;
const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/** How many of each period make a month, for adding them all up. */
const PER_MONTH: Record<SubscriptionPeriod, number> = {
  week: 52 / 12,
  month: 1,
  quarter: 1 / 3,
  half_year: 1 / 6,
  year: 1 / 12,
};

const PERIODS: { value: SubscriptionPeriod; label: string; every: string }[] = [
  { value: "week", label: tx("Weekly"), every: "/wk" },
  { value: "month", label: tx("Monthly"), every: "/mo" },
  { value: "quarter", label: tx("Every 3 months"), every: "/3 mo" },
  { value: "half_year", label: tx("Every 6 months"), every: "/6 mo" },
  { value: "year", label: tx("Yearly"), every: "/yr" },
];

const CURRENCIES = ["USD", "EUR", "GBP", "CAD", "AUD", "INR", "BDT", "PKR", "JPY", "SGD", "AED", "CHF", "BRL"];

/** "$12.99", in this person's way of writing money. */
export function money(amount: number, currency: string, whole = false): string {
  try {
    return new Intl.NumberFormat(language, {
      style: "currency",
      currency,
      maximumFractionDigits: whole ? 0 : 2,
      minimumFractionDigits: whole ? 0 : undefined,
    }).format(amount);
  } catch {
    return `${currency} ${amount.toFixed(whole ? 0 : 2)}`;
  }
}

/** "$9", "$9.99": cents only when there are some, as prices are usually written. */
function short(amount: number, currency: string): string {
  return money(amount, currency, Math.round(amount * 100) % 100 === 0);
}

/** The company's own domain, not the one its mail is sent from: "apple.com", not
 *  "email.apple.com". Two-part endings such as "co.uk" keep the name before them. */
function mainDomain(host: string): string {
  const parts = host.toLowerCase().split(".").filter(Boolean);
  const twoPart = parts.length > 2 && parts[parts.length - 1].length === 2 && ["co", "com", "net", "org", "ac", "gov"].includes(parts[parts.length - 2]);
  return parts.slice(twoPart ? -3 : -2).join(".");
}

/** What it costs a month, or null if its price isn't known. */
function monthly(s: Subscription): number | null {
  if (s.amount === null) return null;
  return s.amount * PER_MONTH[s.period ?? "month"];
}

/** Still being paid for, so counted in the totals. */
const counts = (s: Subscription) => s.status === "active" || s.status === "failed";

function dayStart(unix: number): number {
  const d = new Date(unix * 1000);
  d.setHours(0, 0, 0, 0);
  return d.getTime() / 1000;
}

/** "Today", "Tomorrow", "In 5 days" or a date. */
function whenText(unix: number): string {
  const days = Math.round((dayStart(unix) - dayStart(Date.now() / 1000)) / DAY);
  if (days === 0) return tx("Today");
  if (days === 1) return tx("Tomorrow");
  if (days > 1 && days <= 14) return tx("In {days} days", { days });
  return dateText(unix);
}

function dateText(unix: number): string {
  const d = new Date(unix * 1000);
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString(language, sameYear ? { month: "short", day: "numeric" } : { year: "numeric", month: "short", day: "numeric" });
}

function priceText(s: Subscription): string {
  if (s.amount === null || !s.currency) return tx("Price not found");
  const every = PERIODS.find((p) => p.value === s.period)?.every;
  return every ? `${short(s.amount, s.currency)}${every}` : short(s.amount, s.currency);
}

/** The line under its name: when it renews, or why it doesn't. */
function statusText(s: Subscription): string {
  switch (s.status) {
    case "failed":
      return tx("Payment failed. Update your card or let it end.");
    case "ending":
      return s.ends ? tx("Cancelled. Paid until {date}", { date: dateText(s.ends) }) : tx("Cancelled");
    case "cancelled":
      return tx("Cancelled");
    case "stopped":
      return s.last_paid ? tx("Stopped. Last paid {date}", { date: dateText(s.last_paid) }) : tx("Stopped");
    case "unsure":
      return s.last_paid
        ? tx("No payment since {date}", { date: dateText(s.last_paid) })
        : tx("No recent payment");
    default:
      if (!s.next) return s.last_paid ? tx("Last paid {date}", { date: dateText(s.last_paid) }) : tx("Renewal date not known");
      return s.next_sure
        ? tx("Renews {date}", { date: dateText(s.next) })
        : tx("Next payment around {date}", { date: dateText(s.next) });
  }
}

type Draft = { name: string; amount: string; currency: string; period: SubscriptionPeriod; next: string };

function toDateInput(unix: number | null): string {
  if (!unix) return "";
  const d = new Date(unix * 1000);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;
}

function fromDateInput(text: string): number | null {
  const [y, m, d] = text.split("-").map(Number);
  if (!y || !m || !d) return null;
  return new Date(y, m - 1, d, 12).getTime() / 1000;
}

/** The form for changing one, or adding one that sends no emails. */
function EditDialog({
  sub,
  mainCurrency,
  onSave,
  onClose,
}: {
  sub: Subscription | null;
  mainCurrency: string;
  onSave: (draft: Draft) => Promise<void>;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState<Draft>({
    name: sub?.name ?? "",
    amount: sub?.amount != null ? String(sub.amount) : "",
    currency: sub?.currency ?? mainCurrency,
    period: sub?.period ?? "month",
    next: toDateInput(sub?.next ?? null),
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const set = (change: Partial<Draft>) => setDraft((d) => ({ ...d, ...change }));
  const currencies = CURRENCIES.includes(draft.currency) ? CURRENCIES : [draft.currency, ...CURRENCIES];
  return (
    <Dialog
      title={sub ? tx("Edit subscription") : tx("Add a subscription")}
      subtitle={sub ? undefined : tx("For one that sends no emails, like a gym.")}
      width="s"
      onClose={onClose}
      onSubmit={(e) => {
        e.preventDefault();
        const amount = Number(draft.amount.replace(",", "."));
        if (!draft.name.trim()) return setProblem(tx("Add a name."));
        if (!(amount > 0)) return setProblem(tx("Add a price, such as 12.99."));
        setSaving(true);
        onSave(draft)
          .then(onClose)
          .catch((err) => setProblem(say(err)))
          .finally(() => setSaving(false));
      }}
      footer={
        <>
          <button type="button" className="btn btn-secondary" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button type="submit" className="btn btn-primary" disabled={saving}>
            {sub ? tx("Save") : tx("Add")}
          </button>
        </>
      }
    >
      <div className="subs-form">
        <label className="field">
          <span className="field-label">{tx("Name")}</span>
          <input type="text" autoFocus value={draft.name} placeholder="Netflix" onChange={(e) => set({ name: e.target.value })} />
        </label>
        <div className="subs-form-row">
          <label className="field grow">
            <span className="field-label">{tx("Price")}</span>
            <input
              type="text"
              inputMode="decimal"
              className="tnum"
              value={draft.amount}
              placeholder="12.99"
              onChange={(e) => set({ amount: e.target.value })}
            />
          </label>
          <div className="field">
            <span className="field-label">{tx("Currency")}</span>
            <Choice
              label={tx("Currency")}
              value={draft.currency}
              options={currencies.map((c) => ({ value: c, label: c }))}
              onChange={(currency) => set({ currency })}
            />
          </div>
        </div>
        <div className="subs-form-row subs-form-pair">
          <div className="field">
            <span className="field-label">{tx("How often")}</span>
            <Choice
              label={tx("How often")}
              value={draft.period}
              options={PERIODS.map((p) => ({ value: p.value, label: p.label }))}
              onChange={(period) => set({ period: period as SubscriptionPeriod })}
            />
          </div>
          <div className="field">
            <span className="field-label">
              {tx("Next payment")} <span className="field-optional">{tx("(optional)")}</span>
            </span>
            <DateField label={tx("Next payment")} value={draft.next} onChange={(next) => set({ next })} />
          </div>
        </div>
      </div>
      <Problem text={problem} />
    </Dialog>
  );
}

/** Looks up how to cancel it, in the browser: every service hides it somewhere different. */
function openHowToCancel(name: string) {
  const link = document.createElement("a");
  link.href = `https://www.google.com/search?q=${encodeURIComponent(`how to cancel ${name} subscription`)}`;
  link.target = "_blank";
  link.rel = "noreferrer";
  document.body.appendChild(link);
  link.click();
  link.remove();
}

/** What's known about one subscription, above its own emails, once it's opened. */
export function SubscriptionDetails({ sub, accounts }: { sub: Subscription; accounts: Account[] }) {
  const account = accounts.find((a) => a.id === sub.account_id);
  const month = monthly(sub);
  const host = sub.addresses[0]?.split("@")[1];
  const from = host ? mainDomain(host) : undefined;
  const facts: { label: string; value: string; sub?: string }[] = [
    {
      label: tx("Price"),
      value: priceText(sub),
      sub:
        sub.period && sub.period !== "month" && month !== null && sub.currency
          ? `${short(month, sub.currency)}/mo`
          : undefined,
    },
    {
      label: sub.status === "ending" ? tx("Paid until") : tx("Next payment"),
      value:
        sub.status === "ending" && sub.ends
          ? dateText(sub.ends)
          : sub.next
            ? dateText(sub.next)
            : "—",
      sub: sub.next && sub.status !== "ending" ? (sub.next_sure ? whenText(sub.next) : tx("Worked out from past payments")) : undefined,
    },
    {
      label: tx("Last paid"),
      value: sub.last_paid ? dateText(sub.last_paid) : "—",
      sub: sub.payments > 0 ? tx("{n} payments found", { n: sub.payments }) : undefined,
    },
    {
      label: tx("Provider"),
      value: sub.manual ? tx("Added by you") : (from ?? "—"),
      sub: account ? account.prefs.label || account.email : undefined,
    },
  ];
  return (
    <div className="subs-detail">
      <div className="subs-detail-head">
        <Avatar name={sub.name} email={sub.addresses[0]} size="m" />
        <span className={`subs-detail-status grow truncate-1 ${sub.status === "failed" ? "subs-when-bad" : ""}`}>
          {sub.status === "failed" && <Icon icon={AlertCircleIcon} size="xs" />}
          {statusText(sub)}
        </span>
        <button type="button" className="btn btn-secondary btn-sm" onClick={() => openHowToCancel(sub.name)}>
          <Icon icon={LinkSquare02Icon} size="s" />
          {tx("How to cancel")}
        </button>
      </div>
      <div className="subs-detail-facts">
        {facts.map((f) => (
          <div key={f.label} className="subs-fact">
            <span className="subs-card-label">{f.label}</span>
            <span className="subs-fact-value tnum truncate-1">{f.value}</span>
            {f.sub && <span className="subs-card-sub">{f.sub}</span>}
          </div>
        ))}
      </div>
      {!sub.manual && <div className="day-label subs-detail-mail">{tx("Its emails")}</div>}
    </div>
  );
}

/** A summary card's top line: a small icon and what the number is. */
function CardHead({
  icon,
  label,
  children,
}: {
  icon: Parameters<typeof Icon>[0]["icon"];
  label: string;
  children?: React.ReactNode;
}) {
  return (
    <span className="subs-card-head">
      <span className="subs-card-icon">
        <Icon icon={icon} size="s" />
      </span>
      <span className="subs-card-label">{label}</span>
      {children}
    </span>
  );
}

function Row({
  sub,
  accounts,
  onEdit,
  onSet,
  onRemove,
  onEmails,
}: {
  sub: Subscription;
  accounts: Account[];
  onEdit: () => void;
  onSet: (edit: SubscriptionEdit) => void;
  onRemove: () => void;
  onEmails?: () => void;
}) {
  const account = accounts.find((a) => a.id === sub.account_id);
  const soon = sub.status === "active" && sub.next !== null && sub.next - Date.now() / 1000 < 7 * DAY;
  const month = monthly(sub);
  return (
    <div className="subs-row" data-status={sub.status} data-opens={onEmails ? "" : undefined} onClick={onEmails}>
      <Avatar name={sub.name} email={sub.addresses[0]} size="m" />
      <div className="subs-main">
        <span className="subs-name">
          <span className="truncate-1">{sub.name}</span>
          {soon && sub.next && <span className="subs-soon">{whenText(sub.next)}</span>}
          {account && accounts.length > 1 && <AccountDot email={account.email} hue={accountHue(account)} />}
        </span>
        <span className={`subs-when truncate-1 ${sub.status === "failed" ? "subs-when-bad" : ""}`}>
          {sub.status === "failed" && <Icon icon={AlertCircleIcon} size="xs" />}
          {statusText(sub)}
        </span>
        {sub.status === "unsure" && (
          <div className="subs-ask" onClick={(e) => e.stopPropagation()}>
            <span className="subs-ask-q">{tx("Still paying for it?")}</span>
            <div className="subs-ask-pair" role="group" aria-label={tx("Still paying for {name}?", { name: sub.name })}>
              <button type="button" className="subs-ask-btn" onClick={() => onSet({ status: "active" })}>
                <Icon icon={Tick02Icon} size="xs" />
                {tx("Yes")}
              </button>
              <button type="button" className="subs-ask-btn" onClick={() => onSet({ status: "cancelled" })}>
                <Icon icon={Cancel01Icon} size="xs" />
                {tx("No, cancelled")}
              </button>
            </div>
          </div>
        )}
      </div>
      <div className="subs-price">
        <span className="subs-amount tnum">{priceText(sub)}</span>
        {sub.period && sub.period !== "month" && month !== null && sub.currency && (
          <span className="subs-per tnum">
            {`${short(month, sub.currency)}/mo`}
          </span>
        )}
      </div>
      <div onClick={(e) => e.stopPropagation()}>
        <Menu
          label={tx("More for {name}", { name: sub.name })}
          align="end"
          trigger={(props) => (
            <button {...props} type="button" className="icon-btn icon-btn-sm" aria-label={tx("More")} data-tip={tx("More")}>
              <Icon icon={MoreHorizontalIcon} size="s" />
            </button>
          )}
        >
          {(close) => {
            const done = sub.status === "cancelled" || sub.status === "ending" || sub.status === "stopped";
            const run = (what: () => void) => () => {
              close();
              what();
            };
            return (
              <>
                <MenuItem icon={Edit02Icon} onClick={run(onEdit)}>
                  {tx("Edit price or date")}
                </MenuItem>
                {onEmails && !sub.manual && (
                  <MenuItem icon={Mail01Icon} onClick={run(onEmails)}>
                    {tx("Show its emails")}
                  </MenuItem>
                )}
                <div className="menu-sep" />
                {sub.status === "failed" && (
                  <MenuItem icon={Tick02Icon} onClick={run(() => onSet({ status: "active" }))}>
                    {tx("Payment is sorted")}
                  </MenuItem>
                )}
                {done ? (
                  <MenuItem icon={RepeatIcon} onClick={run(() => onSet({ status: "active" }))}>
                    {tx("Still paying for it")}
                  </MenuItem>
                ) : (
                  <MenuItem icon={Cancel01Icon} onClick={run(() => onSet({ status: "cancelled" }))}>
                    {tx("Already cancelled")}
                  </MenuItem>
                )}
                {!done && (
                  <MenuItem icon={LinkSquare02Icon} onClick={run(() => openHowToCancel(sub.name))}>
                    {tx("How to cancel it")}
                  </MenuItem>
                )}
                <div className="menu-sep" />
                {sub.manual ? (
                  <MenuItem icon={Delete02Icon} danger onClick={run(onRemove)}>
                    {tx("Remove")}
                  </MenuItem>
                ) : (
                  <MenuItem icon={ViewOffSlashIcon} onClick={run(() => onSet({ hidden: true }))}>
                    {tx("Not a subscription")}
                  </MenuItem>
                )}
              </>
            );
          }}
        </Menu>
      </div>
    </div>
  );
}

type Sort = "renewal" | "price_high" | "price_low" | "name";
const SORTS: { value: Sort; label: string }[] = [
  { value: "renewal", label: tx("Next renewal") },
  { value: "price_high", label: tx("Price: high to low") },
  { value: "price_low", label: tx("Price: low to high") },
  { value: "name", label: tx("Name") },
];
/** Kept on this Mac, so the list comes back in the order last chosen. */
const SORT_KEY = "subscriptions-sort";

function savedSort(): Sort {
  try {
    const v = localStorage.getItem(SORT_KEY);
    return SORTS.some((o) => o.value === v) ? (v as Sort) : "renewal";
  } catch {
    return "renewal";
  }
}

/** In the chosen order. Prices are compared by what they cost a month. */
function sorted(list: Subscription[], by: Sort): Subscription[] {
  const name = (a: Subscription, b: Subscription) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  const price = (s: Subscription) => monthly(s);
  return [...list].sort((a, b) => {
    switch (by) {
      case "name":
        return name(a, b);
      case "price_high":
      case "price_low": {
        // Ones without a price go last either way.
        const pa = price(a);
        const pb = price(b);
        if (pa === null || pb === null) return pa === pb ? name(a, b) : pa === null ? 1 : -1;
        return (by === "price_high" ? pb - pa : pa - pb) || name(a, b);
      }
      default: {
        const when = (s: Subscription) => s.next ?? s.ends ?? Number.MAX_SAFE_INTEGER;
        return when(a) - when(b) || name(a, b);
      }
    }
  });
}

/** What it all adds up to, per currency, biggest first. */
const PERIOD_DAYS: Record<SubscriptionPeriod, number> = { week: 7, month: 30, quarter: 91, half_year: 182, year: 365 };

/** What was being paid each month for the last six, in one currency, worked out from when each
 *  subscription's payments began and ended. The current month is the total shown on the card. */
function monthHistory(items: Subscription[], currency: string, now: number): { start: number; total: number }[] {
  const today = new Date(now * 1000);
  const months: { start: number; total: number }[] = [];
  for (let back = 5; back >= 0; back--) {
    const start = new Date(today.getFullYear(), today.getMonth() - back, 1).getTime() / 1000;
    const end = new Date(today.getFullYear(), today.getMonth() - back + 1, 1).getTime() / 1000;
    let total = 0;
    for (const s of items) {
      const m = monthly(s);
      if (m === null || s.currency !== currency) continue;
      // The same ones the card counts: paying now, or a payment that failed.
      if (back === 0) {
        if (counts(s)) total += m;
        continue;
      }
      // Past months count what was being paid then. One with no payment to date it (added by
      // hand, or known only from a reminder) counts for every month, so it can't look new.
      // One that's ending or unsure isn't counted now, so it isn't counted before either,
      // or the tag would show a drop that's only it leaving the count.
      if (s.status === "ending" || s.status === "unsure") continue;
      const began = s.first_paid ?? s.last_paid ?? -Infinity;
      const stopped = counts(s)
        ? Infinity
        : s.last_paid !== null
          ? s.last_paid + PERIOD_DAYS[s.period ?? "month"] * DAY
          : began;
      if (began < end && stopped >= start) total += m;
    }
    months.push({ start, total });
  }
  return months;
}

function monthName(unix: number): string {
  return new Date(unix * 1000).toLocaleDateString(language, { month: "short" });
}

function totals(items: Subscription[]): { currency: string; month: number; count: number }[] {
  const by = new Map<string, { month: number; count: number }>();
  for (const s of items) {
    const m = monthly(s);
    if (!counts(s) || m === null || !s.currency) continue;
    const t = by.get(s.currency) ?? { month: 0, count: 0 };
    by.set(s.currency, { month: t.month + m, count: t.count + 1 });
  }
  return [...by.entries()].map(([currency, t]) => ({ currency, ...t })).sort((a, b) => b.count - a.count || b.month - a.month);
}

/** The subscriptions side of Purchases: what's paid for again and again, when, and the total. */
export function Subscriptions({
  accounts,
  onReceipts,
  onPhoneNav,
  onEmails,
  onNotice,
  searchRef,
}: {
  accounts: Account[];
  /** Where ⌘F puts the cursor. */
  searchRef?: RefObject<HTMLInputElement | null>;
  /** Back to the receipts list. */
  onReceipts: () => void;
  onPhoneNav?: () => void;
  onEmails: (sub: Subscription) => void;
  onNotice: (text: string) => void;
}) {
  const [found, setFound] = useState<Found | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [editing, setEditing] = useState<Subscription | "new" | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [showCancelled, setShowCancelled] = useState(false);
  const [query, setQuery] = useState("");
  const [sortBy, setSortBy] = useState<Sort>(savedSort);

  useEffect(() => {
    let live = true;
    api
      .subscriptions()
      .then((f) => live && setFound(f))
      .catch((e) => live && setProblem(say(e)));
    return () => {
      live = false;
    };
  }, []);

  const change = (p: Promise<Found>) =>
    p.then(setFound).catch((e) => {
      setProblem(say(e));
      throw e;
    });

  const all = found?.items ?? [];
  const sums = useMemo(() => totals(all), [all]);
  const main = sums[0];
  // The totals stay for everything; searching only narrows the list.
  const words = query.trim().toLowerCase();
  const items = sorted(
    words ? all.filter((s) => `${s.name} ${s.addresses.join(" ")} ${priceText(s)}`.toLowerCase().includes(words)) : all,
    sortBy,
  );
  const needsLook = items.filter((s) => s.status === "failed" || s.status === "unsure");
  const paying = items.filter((s) => s.status === "active");
  const ending = items.filter((s) => s.status === "ending");
  const cancelled = items.filter((s) => s.status === "cancelled" || s.status === "stopped");
  const week = all.filter((s) => s.status === "active").filter((s) => s.next !== null && s.next - Date.now() / 1000 < 7 * DAY);
  week.sort((a, b) => (a.next ?? 0) - (b.next ?? 0));
  // The biggest costs in the main currency, for the split bar and the faces on the cards.
  const paid = all.filter(counts).sort((a, b) => (monthly(b) ?? 0) - (monthly(a) ?? 0));
  const split = main
    ? paid
        .filter((s) => s.currency === main.currency && (monthly(s) ?? 0) > 0)
        .slice(0, 2)
        .map((s) => ({ name: s.name, share: (monthly(s) ?? 0) / main.month }))
    : [];
  const rest = 1 - split.reduce((n, p) => n + p.share, 0);
  const endingCount = all.filter((s) => s.status === "ending").length;
  const history = useMemo(() => (main ? monthHistory(all, main.currency, Date.now() / 1000) : []), [all, main]);
  const highest = Math.max(...history.map((h) => h.total), 0);
  const sinceLast = history.length > 1 ? history[history.length - 1].total - history[history.length - 2].total : 0;
  const weekTotal = main ? week.filter((s) => s.currency === main.currency).reduce((n, s) => n + (s.amount ?? 0), 0) : 0;

  const row = (s: Subscription) => (
    <Row
      key={s.key}
      sub={s}
      accounts={accounts}
      onEdit={() => setEditing(s)}
      onSet={(edit) => {
        // Added to what the person already said about it.
        void change(api.subscriptionEdit(s.key, edit)).then(() => {
          if (edit.hidden) onNotice(tx("{name} won't show as a subscription.", { name: s.name }));
        });
      }}
      onRemove={() => void change(api.subscriptionRemove(s.key))}
      onEmails={() => onEmails(s)}
    />
  );

  const section = (label: string, list: Subscription[], extra?: React.ReactNode) =>
    list.length > 0 && (
      <section className="subs-section" aria-label={label}>
        <div className="day-label">
          {label}
          {extra}
        </div>
        {list.map(row)}
      </section>
    );

  return (
    <main className="newsletters subs">
      <div className="list-head news-head" data-tauri-drag-region="deep">
        {onPhoneNav && <IconButton icon={Menu01Icon} label={tx("Folders")} small className="phone-only" onClick={onPhoneNav} />}
        <h2 className="list-title">{tx("Purchases")}</h2>
        <div className="list-search">
          <div role="search" className="search">
            <Icon icon={Search01Icon} size="s" />
            <input
              ref={searchRef}
              type="search"
              aria-label={tx("Search subscriptions")}
              placeholder={tx("Search subscriptions")}
              value={query}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape" && query) {
                  e.stopPropagation();
                  setQuery("");
                }
              }}
            />
            {query ? (
              <IconButton icon={Cancel01Icon} label={tx("Clear search")} small onClick={() => setQuery("")} />
            ) : (
              <span className="kbd-quiet">{keys("⌘F")}</span>
            )}
          </div>
        </div>
        <Choice
          label={tx("Sort by")}
          className="btn btn-secondary btn-sm subs-sort"
          value={sortBy}
          options={SORTS}
          onChange={(v) => {
            setSortBy(v as Sort);
            try {
              localStorage.setItem(SORT_KEY, v);
            } catch {
              // Not kept, which only means it starts at Next renewal next time.
            }
          }}
        />
        <button type="button" className="btn btn-secondary btn-sm" onClick={() => setEditing("new")}>
          <Icon icon={Add01Icon} size="s" />
          {tx("Add")}
        </button>
      </div>
      {/* The same row of tabs, in the same place, as on Receipts. */}
      <div className="purchase-tabs">
        <PurchaseTabs on="subscriptions" onReceipts={onReceipts} />
      </div>
      <Problem text={problem} />
      <div className="list-scroll subs-scroll">
        {!found ? (
          <div className="empty" role="status" aria-label={tx("Loading")}>
            <Spinner />
          </div>
        ) : all.length === 0 ? (
          <div className="empty">
            <div className="empty-icon">
              <Icon icon={RepeatIcon} size="l" />
            </div>
            <div className="empty-title">{tx("No subscriptions found yet")}</div>
            <div className="empty-text">
              {tx("They show here from your receipts and renewal emails. Add any that don't send emails.")}
            </div>
          </div>
        ) : (
          <>
            <div className="subs-cards">
              <div className="subs-card subs-card-hero">
                <CardHead icon={DollarCircleIcon} label={tx("Per month")}>
                  {main && history.length > 1 && Math.abs(sinceLast) >= 1 && (
                    <span className="subs-change tnum" data-down={sinceLast < 0 || undefined}>
                      {sinceLast > 0 ? "▲" : "▼"}{" "}
                      {tx("{amount} vs {month}", {
                        amount: money(Math.abs(sinceLast), main.currency, true),
                        month: monthName(history[history.length - 2].start),
                      })}
                    </span>
                  )}
                </CardHead>
                <div className="subs-card-body">
                  <span className="subs-card-number tnum">{main ? money(main.month, main.currency) : "–"}</span>
                  {main && (
                    <span className="subs-card-sub tnum">
                      {sums.length > 1
                        ? `+ ${sums.slice(1).map((t) => money(t.month, t.currency)).join(" + ")}`
                        : tx("About {amount} a day", { amount: money((main.month * 12) / 365, main.currency) })}
                    </span>
                  )}
                </div>
                {main && highest > 0 && (
                  <div className="subs-card-foot">
                    <div className="subs-bars" role="img" aria-label={tx("Paid each month")}>
                      {history.map((h, i) => (
                        <span
                          key={h.start}
                          className="subs-bar"
                          data-now={i === history.length - 1 || undefined}
                          style={{ "--share": Math.max(h.total / highest, 0.06) } as React.CSSProperties}
                          data-tip={`${monthName(h.start)} · ${money(h.total, main.currency)}`}
                        />
                      ))}
                    </div>
                    <div className="subs-bar-names" aria-hidden>
                      {history.map((h) => (
                        <span key={h.start}>{monthName(h.start)}</span>
                      ))}
                    </div>
                  </div>
                )}
              </div>
              <div className="subs-card">
                <CardHead icon={Calendar03Icon} label={tx("Per year")} />
                <div className="subs-card-body">
                  <span className="subs-card-number tnum">{main ? money(main.month * 12, main.currency, true) : "–"}</span>
                  {sums.length > 1 && (
                    <span className="subs-card-sub tnum">
                      + {sums.slice(1).map((t) => money(t.month * 12, t.currency, true)).join(" + ")}
                    </span>
                  )}
                </div>
                {split.length > 0 && (
                  <div className="subs-card-foot">
                    <div className="subs-split" aria-hidden>
                      {split.map((p, i) => (
                        <span key={p.name} data-part={i} style={{ "--share": p.share } as React.CSSProperties} />
                      ))}
                      {rest > 0.005 && <span data-part="rest" style={{ "--share": rest } as React.CSSProperties} />}
                    </div>
                    <div className="subs-split-keys">
                      {split.map((p, i) => (
                        <span key={p.name} data-part={i} className="truncate-1">
                          {p.name}
                        </span>
                      ))}
                      {rest > 0.005 && <span data-part="rest">{tx("Other")}</span>}
                    </div>
                  </div>
                )}
              </div>
              <div className="subs-card">
                <CardHead icon={RepeatIcon} label={tx("Paying for")} />
                <div className="subs-card-body">
                  <span className="subs-card-number tnum">{paid.length}</span>
                  <span className="subs-card-sub">
                    {endingCount > 0
                      ? tx("subscriptions · {count} ending", { count: endingCount })
                      : tx("subscriptions")}
                  </span>
                </div>
                {paid.length > 0 && (
                  <div className="subs-card-foot subs-faces">
                    <span className="subs-faces-stack">
                      {paid.slice(0, 5).map((s) => (
                        <Avatar key={s.key} name={s.name} email={s.addresses[0]} size="s" />
                      ))}
                    </span>
                    {paid.length > 5 && (
                      <span className="subs-card-sub">{tx("+{count} more", { count: paid.length - 5 })}</span>
                    )}
                  </div>
                )}
              </div>
              <div className="subs-card">
                <CardHead icon={ArrowRight01Icon} label={tx("Next 7 days")} />
                <div className="subs-card-body">
                  <span className="subs-card-number tnum">{main && weekTotal > 0 ? money(weekTotal, main.currency) : week.length}</span>
                  <span className="subs-card-sub">
                    {week.length === 0 ? tx("Nothing renews") : tx("{count} renewing", { count: week.length })}
                  </span>
                </div>
                {week.length > 0 && (
                  <div className="subs-card-foot subs-next">
                    {week.slice(0, 2).map((s) => (
                      <span key={s.key} className="subs-next-row">
                        <Avatar name={s.name} email={s.addresses[0]} size="xs" />
                        <span className="subs-next-name truncate-1">{s.name}</span>
                        {s.next && <span className="subs-next-date tnum">{dateText(s.next)}</span>}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            </div>
            {items.length === 0 && (
              <div className="empty">
                <div className="empty-title">{tx("No subscriptions match")}</div>
                <div className="empty-text">{tx("Try another name, or clear the search.")}</div>
              </div>
            )}
            {section(tx("Needs a look"), needsLook)}
            {section(tx("Renewing"), paying)}
            {section(tx("Ending"), ending)}
            {cancelled.length > 0 && (
              <section className="subs-section" aria-label={tx("Cancelled")}>
                <button type="button" className="day-label subs-toggle" onClick={() => setShowCancelled((v) => !v)}>
                  {tx("Cancelled")} · {cancelled.length}
                  <Icon icon={showCancelled ? ViewOffSlashIcon : ViewIcon} size="xs" />
                </button>
                {showCancelled && cancelled.map(row)}
              </section>
            )}
          </>
        )}
        {found && found.hidden.length > 0 && (
          <section className="subs-section" aria-label={tx("Not subscriptions")}>
            <button type="button" className="day-label subs-toggle" onClick={() => setShowHidden((v) => !v)}>
              {tx("Not subscriptions")} · {found.hidden.length}
              <Icon icon={showHidden ? ViewOffSlashIcon : ViewIcon} size="xs" />
            </button>
            {showHidden &&
              found.hidden.map((s) => (
                <div key={s.key} className="subs-row subs-row-hidden">
                  <Avatar name={s.name} email={s.addresses[0]} size="m" />
                  <div className="subs-main">
                    <span className="subs-name truncate-1">{s.name}</span>
                    <span className="subs-when truncate-1">{priceText(s)}</span>
                  </div>
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    onClick={() => void change(api.subscriptionEdit(s.key, { hidden: false }))}
                  >
                    <Icon icon={Tick02Icon} size="s" />
                    {tx("It is one")}
                  </button>
                </div>
              ))}
          </section>
        )}
        {found && all.length > 0 && (
          <p className="subs-foot">
            {tx("Worked out from your receipts and renewal emails, on this Mac. Prices and dates may be off, so check anything that matters.")}
          </p>
        )}
      </div>
      {editing && (
        <EditDialog
          sub={editing === "new" ? null : editing}
          mainCurrency={main?.currency ?? "USD"}
          onClose={() => setEditing(null)}
          onSave={async (d) => {
            const amount = Number(d.amount.replace(",", "."));
            const next = fromDateInput(d.next);
            if (editing === "new") {
              await change(api.subscriptionAdd(d.name.trim(), amount, d.currency, d.period, next));
            } else {
              const s = editing;
              await change(
                api.subscriptionEdit(s.key, {
                  name: d.name.trim() !== s.name ? d.name.trim() : undefined,
                  amount: amount !== s.amount ? amount : undefined,
                  currency: d.currency !== s.currency ? d.currency : undefined,
                  period: d.period !== s.period ? d.period : undefined,
                  next: d.next !== toDateInput(s.next) && next ? next : undefined,
                }),
              );
            }
          }}
        />
      )}
    </main>
  );
}

/** Receipts or subscriptions, at the top of Purchases. */
export function PurchaseTabs({
  on,
  onReceipts,
  onSubscriptions,
}: {
  on: "receipts" | "subscriptions";
  onReceipts?: () => void;
  onSubscriptions?: () => void;
}) {
  return (
    <div role="tablist" aria-label={tx("Purchases")} className="tabs purchase-tab-bar">
      <button type="button" role="tab" className="tab" aria-selected={on === "receipts"} onClick={onReceipts}>
        {tx("Receipts")}
      </button>
      <button type="button" role="tab" className="tab" aria-selected={on === "subscriptions"} onClick={onSubscriptions}>
        {tx("Subscriptions")}
      </button>
    </div>
  );
}

