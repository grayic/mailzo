import { useEffect, useMemo, useState, type RefObject } from "react";
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
  Delete02Icon,
  Edit02Icon,
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
  { value: "week", label: tx("Weekly"), every: tx("a week") },
  { value: "month", label: tx("Monthly"), every: tx("a month") },
  { value: "quarter", label: tx("Every 3 months"), every: tx("every 3 months") },
  { value: "half_year", label: tx("Every 6 months"), every: tx("every 6 months") },
  { value: "year", label: tx("Yearly"), every: tx("a year") },
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
  return every ? `${money(s.amount, s.currency)} ${every}` : money(s.amount, s.currency);
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
        <div className="field">
          <span className="field-label">{tx("How often")}</span>
          <Choice
            label={tx("How often")}
            value={draft.period}
            options={PERIODS.map((p) => ({ value: p.value, label: p.label }))}
            onChange={(period) => set({ period: period as SubscriptionPeriod })}
          />
        </div>
        <label className="field">
          <span className="field-label">
            {tx("Next payment")} <span className="field-optional">{tx("(optional)")}</span>
          </span>
          <input type="date" value={draft.next} onChange={(e) => set({ next: e.target.value })} />
        </label>
      </div>
      <Problem text={problem} />
    </Dialog>
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
        <span className="subs-name truncate-1">
          {sub.name}
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
      {soon && sub.next && <span className="subs-soon">{whenText(sub.next)}</span>}
      <div className="subs-price">
        <span className="subs-amount tnum">{priceText(sub)}</span>
        {sub.period && sub.period !== "month" && month !== null && sub.currency && (
          <span className="subs-per tnum">
            {tx("{amount} a month", { amount: money(month, sub.currency) })}
          </span>
        )}
      </div>
      <div onClick={(e) => e.stopPropagation()}>
        <Menu
          label={tx("More for {name}", { name: sub.name })}
          align="end"
          trigger={(props) => <IconButton {...props} icon={MoreHorizontalIcon} label={tx("More")} small />}
        >
          {(close) => (
            <>
              <MenuItem icon={Edit02Icon} onClick={() => { close(); onEdit(); }}>
                {tx("Edit")}
              </MenuItem>
              {onEmails && (
                <MenuItem icon={Mail01Icon} onClick={() => { close(); onEmails(); }}>
                  {tx("Show its emails")}
                </MenuItem>
              )}
              {sub.status === "cancelled" || sub.status === "ending" || sub.status === "stopped" ? (
                <MenuItem icon={RepeatIcon} onClick={() => { close(); onSet({ status: "active" }); }}>
                  {tx("Still paying for it")}
                </MenuItem>
              ) : (
                <MenuItem icon={Cancel01Icon} onClick={() => { close(); onSet({ status: "cancelled" }); }}>
                  {tx("Mark as cancelled")}
                </MenuItem>
              )}
              {sub.manual ? (
                <MenuItem icon={Delete02Icon} danger onClick={() => { close(); onRemove(); }}>
                  {tx("Remove")}
                </MenuItem>
              ) : (
                <MenuItem icon={ViewOffSlashIcon} onClick={() => { close(); onSet({ hidden: true }); }}>
                  {tx("Not a subscription")}
                </MenuItem>
              )}
            </>
          )}
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
      onEmails={s.manual || s.addresses.length === 0 ? undefined : () => onEmails(s)}
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
              <span className="kbd-quiet">⌘F</span>
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
              <div className="subs-card">
                <span className="subs-card-label">{tx("Per month")}</span>
                <span className="subs-card-number tnum">{main ? money(main.month, main.currency) : "–"}</span>
                {sums.length > 1 && (
                  <span className="subs-card-sub tnum">
                    + {sums.slice(1).map((t) => money(t.month, t.currency)).join(" + ")}
                  </span>
                )}
              </div>
              <div className="subs-card">
                <span className="subs-card-label">{tx("Per year")}</span>
                <span className="subs-card-number tnum">{main ? money(main.month * 12, main.currency, true) : "–"}</span>
                {sums.length > 1 && (
                  <span className="subs-card-sub tnum">
                    + {sums.slice(1).map((t) => money(t.month * 12, t.currency, true)).join(" + ")}
                  </span>
                )}
              </div>
              <div className="subs-card">
                <span className="subs-card-label">{tx("Paying for")}</span>
                <span className="subs-card-number tnum">{all.filter(counts).length}</span>
                <span className="subs-card-sub">{tx("subscriptions")}</span>
              </div>
              <div className="subs-card">
                <span className="subs-card-label">{tx("Next 7 days")}</span>
                <span className="subs-card-number tnum">{main && weekTotal > 0 ? money(weekTotal, main.currency) : week.length}</span>
                <span className="subs-card-sub">
                  {week.length === 0 ? tx("Nothing renews") : tx("{count} renewing", { count: week.length })}
                </span>
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

