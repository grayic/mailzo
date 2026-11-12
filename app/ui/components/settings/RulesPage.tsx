import { useEffect, useState } from "react";
import { ApiError, api, type Rule } from "../../api";
import { AiMagicIcon, Cancel01Icon, Delete02Icon, FilterHorizontalIcon, PencilEdit02Icon } from "../../icons";
import type { Account, Folder } from "../../types";
import { Confirm, Dialog, Icon, IconButton, Problem, Spinner } from "../ui";
import { Choice, EmptyPage, Page, Row, Section, Toggle } from "./controls";
import { tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/** What happens to the emails, in words. */
function describeThen(rule: Rule): string[] {
  const then: string[] = [];
  const t = rule.then;
  if (t.spam) then.push("move to Spam");
  if (t.move_to) then.push(tx("move to {0}", { "0": t.move_to }));
  if (t.archive) then.push("archive");
  if (t.trash) then.push("delete");
  if (t.mark_read) then.push("mark as read");
  if (t.star) then.push("star");
  if (t.tab) then.push(tx("put on {0}", { "0": t.tab }));
  if (t.mute) then.push("mute");
  return then;
}

/** An AI filter: its words are what it's for. */
const isAi = (rule: Rule) => rule.ai !== null && rule.ai !== undefined;

function describe(rule: Rule): string {
  if (isAi(rule)) {
    const then = describeThen(rule);
    return tx("AI picks the emails, then: {0}.", { "0": then.join(", ") || tx("(say what to do)") });
  }
  const when: string[] = [];
  const w = rule.when;
  if (w.from.length) when.push(`from ${w.from.join(" or ")}`);
  if (w.to.length) when.push(`to ${w.to.join(" or ")}`);
  if (w.subject.length) when.push(tx("with “{0}” in the subject", { "0": w.subject.join("” or “") }));
  if (w.words.length) when.push(`containing “${w.words.join("” or “")}”`);
  if (w.has_attachment) when.push("with files attached");
  if (w.not_from?.length) when.push(tx("not from {0}", { "0": w.not_from.join(" or ") }));
  if (w.not_words?.length) when.push(`without “${w.not_words.join("” or “")}”`);
  const then = describeThen(rule);
  return `Emails ${when.join(", ") || tx("(say which)")}: ${then.join(", ") || tx("(say what to do)")}.`;
}

const words = (text: string) =>
  text
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);

const BLANK: Rule = {
  id: "",
  sentence: "",
  enabled: true,
  inbox: null,
  ai: null,
  when: { from: [], to: [], subject: [], words: [], has_attachment: false, not_from: [], not_words: [] },
  then: { move_to: null, spam: false, archive: false, trash: false, mark_read: false, star: false, tab: null, mute: false },
};

const hasWhen = (r: Rule) =>
  isAi(r) ? !!r.ai?.trim() :
  r.when.from.length + r.when.to.length + r.when.subject.length + r.when.words.length > 0 || r.when.has_attachment;
const hasThen = (r: Rule) =>
  !!r.then.move_to || r.then.spam || r.then.archive || r.then.trash || r.then.mark_read || r.then.star || !!r.then.tab || r.then.mute;

type ConditionKind = "from" | "not_from" | "subject" | "words" | "not_words" | "to" | "has_attachment";
type ActionKind = "spam" | "folder" | "archive" | "trash" | "mark_read" | "star" | "mute" | "tab";
type Condition = { kind: ConditionKind; text: string };
type Action = { kind: ActionKind; value: string };

const CONDITIONS: { kind: ConditionKind; label: string; hint?: string; aria?: string }[] = [
  { kind: "from", label: tx("is from"), hint: "ana@company.com, Stripe", aria: tx("Rule from") },
  { kind: "words", label: tx("mentions"), hint: "crypto, lottery", aria: tx("Rule mentioning") },
  { kind: "subject", label: tx("has in the subject"), hint: "invoice, receipt", aria: tx("Rule subject") },
  { kind: "to", label: tx("is sent to"), hint: "sales@company.com", aria: tx("Rule to") },
  { kind: "not_from", label: tx("isn't from"), hint: "boss@company.com", aria: tx("Rule not from") },
  { kind: "not_words", label: tx("doesn't mention"), hint: "urgent, invoice", aria: tx("Rule not mentioning") },
  { kind: "has_attachment", label: tx("has an attachment") },
];

const ACTIONS: { kind: ActionKind; label: string }[] = [
  { kind: "mark_read", label: tx("Mark as read") },
  { kind: "spam", label: tx("Move to Spam") },
  { kind: "folder", label: tx("Move to folder") },
  { kind: "archive", label: tx("Archive") },
  { kind: "trash", label: tx("Delete") },
  { kind: "star", label: tx("Star") },
  { kind: "mute", label: tx("Mute the conversation") },
  { kind: "tab", label: tx("Put on tab") },
];
const MOVES: ActionKind[] = ["spam", "folder", "archive", "trash"];

const TABS = [
  { value: "general", label: tx("Good to know") },
  { value: "newsletters", label: tx("The rest") },
];

const conditionsOf = (r: Rule): Condition[] => {
  const w = r.when;
  const out: Condition[] = [];
  const add = (kind: ConditionKind, list: string[] | undefined) => {
    if (list?.length) out.push({ kind, text: list.join(", ") });
  };
  add("from", w.from);
  add("words", w.words);
  add("subject", w.subject);
  add("to", w.to);
  add("not_from", w.not_from);
  add("not_words", w.not_words);
  if (w.has_attachment) out.push({ kind: "has_attachment", text: "" });
  return out.length > 0 ? out : [{ kind: "from", text: "" }];
};

const actionsOf = (r: Rule): Action[] => {
  const t = r.then;
  const out: Action[] = [];
  if (t.mark_read) out.push({ kind: "mark_read", value: "" });
  if (t.spam) out.push({ kind: "spam", value: "" });
  if (t.move_to) out.push({ kind: "folder", value: t.move_to });
  if (t.archive) out.push({ kind: "archive", value: "" });
  if (t.trash) out.push({ kind: "trash", value: "" });
  if (t.star) out.push({ kind: "star", value: "" });
  if (t.mute) out.push({ kind: "mute", value: "" });
  if (t.tab) out.push({ kind: "tab", value: t.tab });
  return out.length > 0 ? out : [{ kind: "mark_read", value: "" }];
};

/** The rule these rows add up to. */
const ruleFrom = (base: Rule, conditions: Condition[], actions: Action[]): Rule => {
  const textOf = (kind: ConditionKind) => words(conditions.find((c) => c.kind === kind)?.text ?? "");
  const has = (kind: ActionKind) => actions.some((a) => a.kind === kind);
  const valueOf = (kind: ActionKind) => actions.find((a) => a.kind === kind)?.value || null;
  return {
    ...base,
    when: {
      from: textOf("from"),
      to: textOf("to"),
      subject: textOf("subject"),
      words: textOf("words"),
      not_from: textOf("not_from"),
      not_words: textOf("not_words"),
      has_attachment: conditions.some((c) => c.kind === "has_attachment"),
    },
    then: {
      move_to: valueOf("folder"),
      spam: has("spam"),
      archive: has("archive"),
      trash: has("trash"),
      mark_read: has("mark_read"),
      star: has("star"),
      mute: has("mute"),
      tab: valueOf("tab"),
    },
  };
};

/** Which emails, and what happens to them, as rows of dropdowns. */
function RuleEditor({
  rule,
  accounts,
  foldersBy,
  onChange,
  which,
}: {
  rule: Rule;
  accounts: Account[];
  foldersBy: Record<number, Folder[]>;
  onChange: (rule: Rule) => void;
  /** Shown in place of the conditions, for a filter AI judges. */
  which?: React.ReactNode;
}) {
  const [conditions, setConditions] = useState(() => conditionsOf(rule));
  const [actions, setActions] = useState(() => actionsOf(rule));

  const scope = accounts.find((a) => a.sync_id === rule.inbox);
  const named = [
    ...new Set(
      (scope ? [scope] : accounts).flatMap((a) =>
        (foldersBy[a.id] ?? []).filter((f) => f.role === "folder").map((f) => f.name),
      ),
    ),
  ].sort();
  // A folder typed in a sentence may not be in the list yet.
  for (const a of actions) if (a.kind === "folder" && a.value && !named.includes(a.value)) named.push(a.value);

  const update = (nextConditions: Condition[], nextActions: Action[]) => {
    setConditions(nextConditions);
    setActions(nextActions);
    onChange(ruleFrom(rule, nextConditions, nextActions));
  };
  const setCondition = (i: number, patch: Partial<Condition>) =>
    update(conditions.map((c, n) => (n === i ? { ...c, ...patch } : c)), actions);
  const setAction = (i: number, patch: Partial<Action>) =>
    update(conditions, actions.map((a, n) => (n === i ? { ...a, ...patch } : a)));

  const usedConditions = conditions.map((c) => c.kind);
  const nextCondition = CONDITIONS.find((c) => !usedConditions.includes(c.kind));
  const moving = actions.some((a) => MOVES.includes(a.kind));
  // Only one of the moves makes sense at once, as an email can't go two places.
  const actionChoices = (mine: ActionKind) =>
    ACTIONS.filter(
      (a) =>
        a.kind === mine ||
        (!actions.some((x) => x.kind === a.kind) && !(MOVES.includes(a.kind) && moving && !MOVES.includes(mine))),
    );
  const nextAction = ACTIONS.find(
    (a) => !actions.some((x) => x.kind === a.kind) && !(MOVES.includes(a.kind) && moving),
  );
  const defaultValue = (kind: ActionKind) => (kind === "tab" ? "newsletters" : kind === "folder" ? (named[0] ?? "") : "");

  return (
    <>
      {which ?? (
      <div className="stack">
        <div className="section-title">{tx("If an email")}</div>
        {conditions.map((c, i) => {
          const about = CONDITIONS.find((x) => x.kind === c.kind);
          return (
            <div key={c.kind} className="rule-row">
              <Choice
                label={`Condition ${i + 1}`}
                value={c.kind}
                options={CONDITIONS.filter((x) => x.kind === c.kind || !usedConditions.includes(x.kind)).map((x) => ({
                  value: x.kind,
                  label: x.label,
                }))}
                onChange={(kind) => setCondition(i, { kind: kind as ConditionKind })}
              />
              {about?.aria ? (
                <input
                  aria-label={about.aria}
                  placeholder={about.hint}
                  value={c.text}
                  onChange={(e) => setCondition(i, { text: e.target.value })}
                />
              ) : (
                <span />
              )}
              <IconButton
                icon={Cancel01Icon}
                label={tx("Remove this condition")}
                small
                disabled={conditions.length === 1}
                onClick={() => update(conditions.filter((_, n) => n !== i), actions)}
              />
            </div>
          );
        })}
        {nextCondition && (
          <button
            type="button"
            className="link-btn rule-add"
            onClick={() => update([...conditions, { kind: nextCondition.kind, text: "" }], actions)}
          >
            {tx("Add another condition")}</button>
        )}
        <div className="fine">{tx("Put commas between several, like ana@company.com, Stripe. Every line must match.")}</div>
      </div>
      )}
      <div className="stack">
        <div className="section-title">{tx("Then")}</div>
        {actions.map((a, i) => (
          <div key={a.kind} className="rule-row">
            <Choice
              label={`Action ${i + 1}`}
              value={a.kind}
              options={actionChoices(a.kind).map((x) => ({ value: x.kind, label: x.label }))}
              onChange={(kind) => setAction(i, { kind: kind as ActionKind, value: defaultValue(kind as ActionKind) })}
            />
            {a.kind === "folder" ? (
              named.length > 0 ? (
                <Choice
                  label={tx("Rule folder")}
                  value={a.value}
                  options={named.map((n) => ({ value: n, label: n }))}
                  onChange={(value) => setAction(i, { value })}
                />
              ) : (
                <span className="fine">{tx("Make a folder first, with the + in the sidebar.")}</span>
              )
            ) : a.kind === "tab" ? (
              <Choice label={tx("Rule tab")} value={a.value} options={TABS} onChange={(value) => setAction(i, { value })} />
            ) : (
              <span />
            )}
            <IconButton
              icon={Cancel01Icon}
              label={tx("Remove this action")}
              small
              disabled={actions.length === 1}
              onClick={() => update(conditions, actions.filter((_, n) => n !== i))}
            />
          </div>
        ))}
        {nextAction && (
          <button
            type="button"
            className="link-btn rule-add"
            onClick={() =>
              update(conditions, [...actions, { kind: nextAction.kind, value: defaultValue(nextAction.kind) }])
            }
          >
            {tx("Add another action")}</button>
        )}
      </div>
      {accounts.length > 1 && (
        <div className="field">
          <span className="field-label">{tx("In")}</span>
          <Choice
            label={tx("Rule inbox")}
            value={rule.inbox ?? ""}
            options={[
              { value: "", label: tx("All inboxes") },
              ...accounts.map((a) => ({ value: a.sync_id, label: a.prefs.label || a.email })),
            ]}
            onChange={(id) => onChange({ ...ruleFrom(rule, conditions, actions), inbox: id || null })}
          />
        </div>
      )}
    </>
  );
}

export function RulesPage({ accounts, foldersBy }: { accounts: Account[]; foldersBy: Record<number, Folder[]> }) {
  const [rules, setRules] = useState<Rule[] | null>(null);
  /** The filter open in the window, if any. `run` starts its boxes afresh. */
  const [draft, setDraft] = useState<{ rule: Rule; run: number } | null>(null);
  const [preview, setPreview] = useState<{ count: number; samples: { from: string; subject: string }[] } | null>(null);
  /** What an AI filter picked from the latest emails, once "Try it" is pressed. */
  const [tried, setTried] = useState<{ looked: number; count: number; samples: { from: string; subject: string }[] } | null>(null);
  const [trying, setTrying] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState<{ id: string; count: number } | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const load = () =>
    api
      .rulesList()
      .then(setRules)
      .catch((e) => setError(say(e)));
  useEffect(() => {
    void load();
  }, []);

  const previewOf = (rule: Rule) => {
    setTried(null);
    if (isAi(rule) || !hasWhen(rule)) {
      setPreview(null);
      return;
    }
    api
      .rulePreview(rule)
      .then(setPreview)
      .catch(() => {});
  };

  const open = (rule: Rule) => {
    setDraft((d) => ({ rule, run: (d?.run ?? 0) + 1 }));
    setError(null);
    setNote(null);
    previewOf(rule);
  };

  const close = () => {
    setDraft(null);
    setPreview(null);
    setTried(null);
    setError(null);
  };

  const change = (rule: Rule) => {
    setDraft((d) => (d ? { ...d, rule } : d));
    previewOf(rule);
  };

  /** Normal conditions, or words for AI. What happens to the emails stays as it was. */
  const pickKind = (kind: string) => {
    if (!draft) return;
    const ai = kind === "ai";
    if (ai === isAi(draft.rule)) return;
    // The action shown at the start counts, as it does once a condition is typed.
    const then = hasThen(draft.rule) ? draft.rule.then : ruleFrom(draft.rule, [], actionsOf(draft.rule)).then;
    const rule = { ...draft.rule, then, ai: ai ? "" : null };
    setDraft({ rule, run: draft.run + 1 });
    setError(null);
    previewOf(rule);
  };

  const tryIt = async () => {
    if (!draft || trying) return;
    setTrying(true);
    setError(null);
    try {
      setTried(await api.ruleTryAi(draft.rule));
    } catch (err) {
      setError(say(err));
    } finally {
      setTrying(false);
    }
  };

  const save = async () => {
    // A second click while it saves would make the same filter twice.
    if (!draft || saving) return;
    setSaving(true);
    setError(null);
    try {
      const saved = await api.ruleSave(draft.rule);
      const count = isAi(saved) ? 0 : (preview?.count ?? 0);
      close();
      await load();
      if (count > 0) setApplying({ id: saved.id, count });
      else if (isAi(saved)) setNote(tx("Saved. AI will read each new email before it reaches your inbox."));
      else setNote(tx("Saved. It will apply to new mail from now on."));
    } catch (err) {
      setError(say(err));
    } finally {
      setSaving(false);
    }
  };

  const apply = async (id: string) => {
    if (busy) return;
    setApplying(null);
    setBusy(true);
    try {
      const reply = await api.ruleApplyNow(id);
      setNote(`Done, for ${reply.done} ${reply.done === 1 ? "email" : "emails"}.`);
    } catch (err) {
      setError(say(err));
    } finally {
      setBusy(false);
    }
  };

  const nameOf = (r: Rule) => (isAi(r) ? r.ai?.trim() || describe(r) : r.sentence || describe(r));

  return (
    <Page
      title={tx("Filters")}
      intro={tx("Do something automatically when an email matches.")}
      action={
        <button type="button" className="btn btn-primary btn-sm" onClick={() => open(BLANK)}>
          {tx("New filter")}</button>
      }
    >
      {rules && rules.length > 0 && (
        <Section note={tx("They don't run in inboxes shared with a team.")}>
          {rules.map((r) => (
            <Row key={r.id} label={nameOf(r)} help={isAi(r) || r.sentence ? describe(r) : undefined}>
              <div className="hstack-m">
                <Toggle
                  label={`${nameOf(r)} on`}
                  on={r.enabled}
                  onChange={(on) => {
                    void api
                      .ruleSave({ ...r, enabled: on })
                      .then(load)
                      .catch((e) => setError(say(e)));
                  }}
                />
                <IconButton icon={PencilEdit02Icon} label={tx("Edit filter: {0}", { "0": nameOf(r) })} small onClick={() => open(r)} />
                <IconButton
                  icon={Delete02Icon}
                  label={tx("Delete filter: {0}", { "0": nameOf(r) })}
                  small
                  onClick={() => {
                    void api
                      .ruleDelete(r.id)
                      .then(load)
                      .catch((e) => setError(say(e)));
                  }}
                />
              </div>
            </Row>
          ))}
        </Section>
      )}
      {rules && rules.length === 0 && (
        <EmptyPage title={tx("No filters yet")} text={tx("Move, mark or star emails automatically, like moving anything about crypto to Spam.")} />
      )}
      {!draft && <Problem text={error} />}
      {note && <div role="status" className="fine">{note}</div>}
      {draft && (
        <Dialog
          title={draft.rule.id ? tx("Edit filter") : tx("New filter")}
          width="m"
          onClose={close}
          footer={
            <>
              <button type="button" className="btn btn-ghost" onClick={close}>
                {tx("Cancel")}</button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={saving || !hasWhen(draft.rule) || !hasThen(draft.rule)}
                onClick={() => void save()}
              >
                {saving && <Spinner size="xs" />}
                {tx("Save filter")}</button>
            </>
          }
        >
          <div className="filter-form">
            <div
              role="radiogroup"
              aria-label={tx("Kind of filter")}
              className="kind-switch"
              data-on={isAi(draft.rule) ? "ai" : "normal"}
            >
              <span className="kind-switch-thumb" aria-hidden />
              {(
                [
                  { value: "normal", label: tx("Normal filter"), icon: FilterHorizontalIcon },
                  { value: "ai", label: tx("Filter by AI"), icon: AiMagicIcon },
                ] as const
              ).map((k) => (
                <button
                  key={k.value}
                  type="button"
                  role="radio"
                  aria-checked={(k.value === "ai") === isAi(draft.rule)}
                  className={`kind-switch-option kind-${k.value}`}
                  onClick={() => pickKind(k.value)}
                >
                  <Icon icon={k.icon} size="s" />
                  {k.label}
                </button>
              ))}
            </div>
            <RuleEditor
              key={draft.run}
              rule={draft.rule}
              accounts={accounts}
              foldersBy={foldersBy}
              onChange={change}
              which={
                isAi(draft.rule) ? (
                  <div className="stack">
                    <div className="section-title">{tx("If an email is")}</div>
                    <textarea
                      className="input filter-ai-words"
                      aria-label={tx("Which emails, in your words")}
                      autoFocus
                      rows={3}
                      maxLength={500}
                      value={draft.rule.ai ?? ""}
                      placeholder={tx("Cold sales pitches from people I've never emailed")}
                      onChange={(e) => change({ ...draft.rule, ai: e.target.value })}
                    />
                    <div className="fine">
                      {tx("Say it as you would to a person. AI reads every new email before it reaches your inbox, in a second or two.")}
                    </div>
                  </div>
                ) : undefined
              }
            />
            {isAi(draft.rule) ? (
              <div className="stack">
                <button
                  type="button"
                  className="link-btn rule-add"
                  disabled={trying || !draft.rule.ai?.trim()}
                  onClick={() => void tryIt()}
                >
                  {trying && <Spinner size="xs" />}
                  {tx("Try it on your latest emails")}</button>
                {tried && (
                  <div className="fine">
                    {tried.count === 0
                      ? tx("None of your latest {0} emails fit.", { "0": tried.looked })
                      : tx("Picks {0} of your latest {1} emails, such as {2}", {
                          "0": tried.count,
                          "1": tried.looked,
                          "2": tried.samples
                            .slice(0, 3)
                            .map((s) => `“${s.subject}”`)
                            .join(", "),
                        })}
                  </div>
                )}
              </div>
            ) : null}
            {!isAi(draft.rule) && hasWhen(draft.rule) && (
              <div className="stack">
                {hasThen(draft.rule) && <div className="fine">{describe(draft.rule)}</div>}
                {preview && (
                  <div className="fine">
                    {preview.count === 0
                      ? tx("Nothing in your inbox matches right now.")
                      : (preview.count === 1 ? tx("Matches 1 email in your inbox now, such as {2}", { "2": preview.samples
                          .slice(0, 3)
                          .map((s) => `“${s.subject}”`)
                          .join(", ") }) : tx("Matches {0} emails in your inbox now, such as {2}", { "0": preview.count, "2": preview.samples
                          .slice(0, 3)
                          .map((s) => `“${s.subject}”`)
                          .join(", ") }))}
                  </div>
                )}
              </div>
            )}
            <Problem text={error} />
          </div>
        </Dialog>
      )}
      {applying && (
        <Confirm
          title={`Also do this to the ${applying.count} ${applying.count === 1 ? "email" : "emails"} already in your inbox?`}
          confirm={tx("Yes, do it now")}
          keep={tx("Only new mail")}
          onKeep={() => {
            setApplying(null);
            setNote(tx("Saved. It will apply to new mail from now on."));
          }}
          onConfirm={() => void apply(applying.id)}
        >
          {tx("The filter is saved either way.")}</Confirm>
      )}
    </Page>
  );
}
