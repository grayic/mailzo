import { createContext, useContext, useState } from "react";
import { api } from "./api";
import { Add01Icon, Delete02Icon } from "./icons";
import { Confirm, Dialog, Icon, IconButton, Problem, Spinner } from "./components/ui";
import type { Tag, ThreadTag } from "./types";
import { language, tx } from "./i18n";

/** The colours a tag can have, in the order they're offered. */
export const TAG_COLOURS = ["gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink"] as const;

/** Every tag the person has made, so a row can show a tag's name and colour from its number. */
export const TagsContext = createContext<Tag[]>([]);

export const useTags = () => useContext(TagsContext);

const say = (e: unknown) => (e instanceof Error ? e.message : String(e));

export const amountText = (amount: number) =>
  amount.toLocaleString(language, { maximumFractionDigits: 2 });

/**
 * An amount as people type it: "1,200", "1200.50" or "12,50". A comma is a decimal point
 * only when it's the last mark and one or two digits follow it.
 */
export function readAmount(words: string): number | null {
  let clean = words.replace(/[^\d.,-]/g, "");
  if (clean === "") return null;
  if (!clean.includes(".") && /,\d{1,2}$/.test(clean)) clean = clean.replace(/,(?=\d{1,2}$)/, ".");
  clean = clean.replace(/,/g, "");
  return Number(clean);
}

/** The tags on a conversation, as small coloured labels. */
export function TagPills({ tags }: { tags: ThreadTag[] | undefined }) {
  const all = useTags();
  if (!tags?.length) return null;
  return (
    <>
      {tags.map((t) => {
        const tag = all.find((x) => x.id === t.tag_id);
        if (!tag) return null;
        return (
          <span key={t.tag_id} className="pill tag-pill" data-colour={tag.colour}>
            {tag.name}
            {t.amount != null && <span className="tnum tag-amount-text">{amountText(t.amount)}</span>}
          </span>
        );
      })}
    </>
  );
}

/** A dot in a tag's colour. */
export function TagDot({ colour }: { colour: string }) {
  return <span aria-hidden className="tag-dot" data-colour={colour} />;
}

/** Chooses the tags on one conversation, and makes new ones. */
export function TagPicker({
  thread,
  onClose,
  onChanged,
}: {
  thread: { account_id: number; thread_id: string; tags?: ThreadTag[] };
  onClose: () => void;
  /** Tags or what's on this conversation changed. */
  onChanged: () => void;
}) {
  const all = useTags();
  const [on, setOn] = useState<ThreadTag[]>(thread.tags ?? []);
  const [amounts, setAmounts] = useState<Record<number, string>>(() =>
    Object.fromEntries((thread.tags ?? []).map((t) => [t.tag_id, t.amount == null ? "" : String(t.amount)])),
  );
  const [name, setName] = useState("");
  const [colour, setColour] = useState<string>("blue");
  const [problem, setProblem] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<Tag | null>(null);
  const [adding, setAdding] = useState(false);

  const put = async (tag_id: number, wanted: boolean, amountWords?: string) => {
    setProblem(null);
    const amount = readAmount(amountWords ?? amounts[tag_id] ?? "");
    if (amount !== null && !Number.isFinite(amount)) {
      setProblem(tx("That amount isn't a number."));
      return;
    }
    try {
      setOn(await api.threadTag(thread.account_id, thread.thread_id, tag_id, wanted, amount));
      onChanged();
    } catch (e) {
      setProblem(say(e));
    }
  };

  const add = async (e: React.FormEvent) => {
    e.preventDefault();
    // Pressing Return twice while it saves would make the same tag twice.
    if (!name.trim() || adding) return;
    setAdding(true);
    setProblem(null);
    try {
      const tag = await api.tagSave(null, name, colour);
      setName("");
      await put(tag.id, true, "");
    } catch (e) {
      setProblem(say(e));
    } finally {
      setAdding(false);
    }
  };

  return (
    <Dialog title={tx("Tags")} subtitle={tx("Your own labels, with an amount if you like")} width="s" onClose={onClose}>
      <div className="tag-list">
        {all.length === 0 && <p className="muted">{tx("No tags yet. Make one below, such as Paid or Waiting on client.")}</p>}
        {all.map((tag) => {
          const checked = on.some((t) => t.tag_id === tag.id);
          return (
            <div key={tag.id} className="tag-row">
              <label className="check-row grow">
                <input
                  type="checkbox"
                  className="check"
                  checked={checked}
                  onChange={(e) => void put(tag.id, e.target.checked)}
                />
                <TagDot colour={tag.colour} />
                <span className="truncate-1">{tag.name}</span>
              </label>
              {checked && (
                <input
                  type="text"
                  inputMode="decimal"
                  className="input tag-amount tnum"
                  aria-label={tx("Amount for {0}", { "0": tag.name })}
                  placeholder={tx("Amount")}
                  value={amounts[tag.id] ?? ""}
                  onChange={(e) => setAmounts((a) => ({ ...a, [tag.id]: e.target.value }))}
                  onBlur={(e) => void put(tag.id, true, e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                />
              )}
              <IconButton icon={Delete02Icon} label={tx("Delete the {0} tag", { "0": tag.name })} small onClick={() => setDeleting(tag)} />
            </div>
          );
        })}
      </div>
      <form className="tag-new" onSubmit={(e) => void add(e)}>
        <input
          type="text"
          className="input"
          aria-label={tx("New tag name")}
          placeholder={tx("New tag")}
          maxLength={40}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <div role="radiogroup" aria-label={tx("Colour")} className="tag-colours">
          {TAG_COLOURS.map((c) => (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={colour === c}
              aria-label={c}
              className="tag-swatch"
              data-colour={c}
              onClick={() => setColour(c)}
            />
          ))}
        </div>
        <button type="submit" className="btn btn-secondary btn-sm" disabled={adding || !name.trim()}>
          {adding ? <Spinner size="xs" /> : <Icon icon={Add01Icon} size="s" />}
          {tx("Add tag")}</button>
      </form>
      <Problem text={problem} />
      {deleting && (
        <Confirm
          title={`Delete the ${deleting.name} tag?`}
          confirm={tx("Delete tag")}
          keep={tx("Keep")}
          danger
          onKeep={() => setDeleting(null)}
          onConfirm={() => {
            const gone = deleting;
            setDeleting(null);
            void api
              .tagDelete(gone.id)
              .then(() => {
                setOn((list) => list.filter((t) => t.tag_id !== gone.id));
                onChanged();
              })
              .catch((e) => setProblem(say(e)));
          }}
        >
          {tx("It comes off every conversation. Your emails stay where they are.")}</Confirm>
      )}
    </Dialog>
  );
}
