import { FormEvent, useEffect, useRef, useState } from "react";
import { ApiError, api, type AiPlan, type AiStatus, type StyleProfile } from "../../api";
import { Confirm, Icon, PasswordInput, Problem, Spinner } from "../ui";
import type { Account } from "../../types";
import { useSettings } from "../../settings";
import { Choice, Page, Row, Section, SettingChoice, SettingToggle, Toggle } from "./controls";
import { tx } from "../../i18n";
import { CreditCardIcon, Key01Icon, LaptopIcon } from "../../icons";
import { usePaid } from "../../paid";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

const PLAN_CHECK_MS = 3000;
const PLAN_CHECKS = 60;

/** A way to run AI, with what it costs said at a glance. */
function Tier({ name, cost, free }: { name: string; cost: string; free?: boolean }) {
  return (
    <span className="ai-tier">
      {name}
      <span className={`pill ${free ? "pill-free" : ""}`}>{cost}</span>
    </span>
  );
}

/** A plan the person already pays for, used through the company's own program on this Mac. */
function PlanRows({ onUsed }: { onUsed: (status: AiStatus) => void }) {
  const [plans, setPlans] = useState<AiPlan[] | null>(null);
  const [signingIn, setSigningIn] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Plans whose sign-in turned out to have run out when used.
  const [expired, setExpired] = useState<string[]>([]);

  useEffect(() => {
    void api
      .aiPlans()
      .then(setPlans)
      .catch((e) => setError(say(e)));
  }, []);

  // The sign-in happens on the company's own page, so keep looking until it's done.
  useEffect(() => {
    if (!signingIn) return;
    let checks = 0;
    const timer = setInterval(() => {
      checks += 1;
      void api.aiPlans().then((list) => {
        setPlans(list);
        if (list.find((p) => p.id === signingIn)?.signed_in || checks >= PLAN_CHECKS) setSigningIn(null);
      });
    }, PLAN_CHECK_MS);
    return () => clearInterval(timer);
  }, [signingIn]);

  const signIn = (id: string) => {
    setError(null);
    setExpired((was) => was.filter((p) => p !== id));
    setSigningIn(id);
    void api.aiPlanSignIn(id).catch((e) => {
      setSigningIn(null);
      setError(say(e));
    });
  };

  const use = async (id: string) => {
    setBusy(id);
    setError(null);
    try {
      onUsed(await api.aiPlanUse(id));
    } catch (e) {
      // Its sign-in ran out: the row says so and offers Sign in, right here.
      if (e instanceof ApiError && e.code === "ai_key") {
        setExpired((was) => [...was, id]);
        setPlans((list) => list?.map((p) => (p.id === id ? { ...p, signed_in: false } : p)) ?? null);
      } else {
        setError(say(e));
      }
    } finally {
      setBusy(null);
    }
  };

  return (
    <Section title={<Tier name={tx("AI subscription")} cost={tx("Your plan")} />}>
      {plans?.map((plan) => (
        <Row
          key={plan.id}
          icon={CreditCardIcon}
          label={plan.name}
          help={
            !plan.found
              ? tx("Needs {0} on this Mac", { "0": plan.id === "codex" ? "Codex" : "Claude Code" })
              : signingIn === plan.id
                ? tx("Finish signing in on the page that opened")
                : plan.signed_in
                  ? tx("Signed in")
                  : expired.includes(plan.id)
                    ? tx("Sign-in expired")
                    : tx("Sign in with {0}", { "0": plan.id === "codex" ? "Codex" : "Claude Code" })
          }
        >
          {!plan.found ? (
            <a className="btn btn-secondary btn-sm" href={plan.install_page} target="_blank" rel="noreferrer">
              {tx("Install")}</a>
          ) : plan.signed_in ? (
            <button className="btn btn-secondary btn-sm" disabled={busy !== null} onClick={() => void use(plan.id)}>
              {busy === plan.id && <Spinner size="xs" />}
              {tx("Use")}</button>
          ) : (
            <button className="btn btn-secondary btn-sm" disabled={signingIn !== null} onClick={() => signIn(plan.id)}>
              {signingIn === plan.id && <Spinner size="xs" />}
              {tx("Sign in")}</button>
          )}
        </Row>
      ))}
      {!plans && !error && (
        <div className="group-row">
          <Spinner size="xs" />
        </div>
      )}
      {error && (
        <div className="group-row">
          <Problem text={error} />
        </div>
      )}
    </Section>
  );
}

/** Apple Intelligence: free and on this Mac, used until a plan or key is connected. */
function AppleRow({ state, using }: { state: string | null; using: boolean }) {
  if (!state || state === "not_this_mac") return null;
  return (
    <Section title={<Tier name={tx("On this Mac")} cost={tx("Free")} free />}>
      {state === "ready" ? (
        <SettingToggle
          setting="ai_on_device"
          icon={LaptopIcon}
          label={tx("Apple Intelligence")}
          help={using ? tx("Private. Best for short tasks.") : tx("Private. No sign-in.")}
        />
      ) : (
        <Row
          icon={LaptopIcon}
          label={tx("Apple Intelligence")}
          help={
            state === "turned_off"
              ? tx("Turn it on in System Settings › Apple Intelligence.")
              : tx("Apple is still getting it ready on this Mac.")
          }
        />
      )}
    </Section>
  );
}

/** Which inboxes AI may read. */
function AiLimits({ accounts, onChanged }: { accounts: Account[]; onChanged: () => void }) {
  const [error, setError] = useState<string | null>(null);
  return (
    <Section
      title={tx("What AI may read")}
      note={tx("Never spam, trash, drafts or suspicious email. Shared inboxes start off.")}
    >
      {accounts.map((a) => (
        <Row
          key={a.id}
          label={a.prefs.label || a.email}
          help={a.prefs.team_shared ? tx("Shared with your team. Only affects your own AI.") : undefined}
        >
          <Toggle
            label={tx("AI may read {0}", { "0": a.email })}
            on={a.prefs.team_shared ? !!a.prefs.shared_ai_allowed : a.prefs.ai_allowed}
            onChange={(on) => {
              setError(null);
              void api
                .updateAccountPrefs(
                  a.id,
                  a.prefs.team_shared ? { ...a.prefs, shared_ai_allowed: on } : { ...a.prefs, ai_allowed: on },
                )
                .then(onChanged)
                .catch((e) => setError(say(e)));
            }}
          />
        </Row>
      ))}
      {error && (
        <div className="group-row">
          <Problem text={error} />
        </div>
      )}
    </Section>
  );
}

/** How the person writes, in a few lines they can read and change. Drafts use it. */
function WritingStyle({ accounts }: { accounts: Account[] }) {
  const own = accounts.filter((a) => !a.prefs.team_shared);
  const [accountId, setAccountId] = useState<number | null>(own[0]?.id ?? null);
  const [profile, setProfile] = useState<StyleProfile | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"make" | "save" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (accountId === null) return;
    setError(null);
    setSaved(false);
    void api
      .styleGet(accountId)
      .then((p) => {
        setProfile(p);
        setText(p?.text ?? "");
      })
      .catch((e) => setError(say(e)));
  }, [accountId]);

  if (accountId === null) return null;
  const run = async (what: "make" | "save") => {
    setBusy(what);
    setError(null);
    setSaved(false);
    try {
      const p = what === "make" ? await api.styleMake(accountId) : await api.styleSave(accountId, text);
      setProfile(p);
      setText(p?.text ?? "");
      if (what === "save") setSaved(true);
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(null);
    }
  };
  const changed = text.trim() !== (profile?.text ?? "").trim();

  return (
    <Section
      title={tx("Your writing style")}
      note={
        profile?.edited
          ? tx("Your own words, kept as they are.")
          : tx("Learned from your sent mail, refreshed monthly.")
      }
    >
      {own.length > 1 && (
        <Row label={tx("Inbox")}>
          <Choice
            label={tx("Inbox for the writing style")}
            options={own.map((a) => ({ value: String(a.id), label: a.prefs.label || a.email }))}
            value={String(accountId)}
            onChange={(v) => setAccountId(Number(v))}
          />
        </Row>
      )}
      <div className="group-row style-row">
        <textarea
          className="input style-profile"
          aria-label={tx("How you write")}
          rows={6}
          placeholder={tx("Made the first time AI writes a reply.")}
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            setSaved(false);
          }}
        />
        <div className="hstack-m">
          <button type="button" className="btn btn-secondary btn-sm" disabled={busy !== null || !changed} onClick={() => void run("save")}>
            {busy === "save" && <Spinner size="xs" />}
            {tx("Save")}</button>
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy !== null} onClick={() => void run("make")}>
            {busy === "make" && <Spinner size="xs" />}
            {profile ? tx("Make it again") : tx("Make it from my sent mail")}
          </button>
          {saved && (
            <span role="status" className="fine">
              {tx("Saved")}</span>
          )}
        </div>
        <Problem text={error} />
      </div>
    </Section>
  );
}

export function AiPage({
  onChanged,
  accounts,
  onAccountsChanged,
}: {
  onChanged: () => void;
  accounts: Account[];
  onAccountsChanged: () => void;
}) {
  const { settings } = useSettings();
  // Sorting by itself and the spam second opinion come with Mailzo Cloud.
  const paid = usePaid();
  const [spend, setSpend] = useState<{ calls: number; dollars: number | null } | null>(null);
  useEffect(() => {
    api
      .aiSpend()
      .then(setSpend)
      .catch(() => {});
  }, []);
  const [status, setStatus] = useState<AiStatus | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);

  // Read again when Apple Intelligence is switched on or off, as that changes what answers.
  const firstRead = useRef(true);
  useEffect(() => {
    api
      .aiStatus()
      .then((next) => {
        setStatus(next);
        if (!firstRead.current) onChanged();
        firstRead.current = false;
      })
      .catch((e) => setError(say(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings.ai_on_device]);

  const serviceId = picked ?? status?.service ?? "anthropic";
  const service = status?.services.find((s) => s.id === serviceId);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.aiSetKey(key, serviceId));
      setKey("");
      setPicked(null);
      onChanged();
    } catch (err) {
      setError(say(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setRemoving(false);
    setError(null);
    try {
      setStatus(await api.aiRemoveKey());
      onChanged();
    } catch (err) {
      setError(say(err));
    }
  };

  const chooseModel = async (model: string) => {
    setError(null);
    try {
      setStatus(await api.aiChooseModel(model));
    } catch (err) {
      setError(say(err));
    }
  };

  return (
    <Page
      title={tx("AI")}
      intro={
        status?.included
          ? tx("AI is included. You can use your own plan or key instead.")
          : status?.using_on_device
            ? tx("Free on this Mac, or connect a bigger AI.")
            : tx("Use your own AI plan or key. You pay them directly.")
      }
    >
      {status?.included && (
        <Section title={tx("AI service")}>
          <Row label={tx("Included with Mailzo Cloud")} help={tx("Nothing to set up.")} />
        </Section>
      )}
      {status && status.via !== "key" ? (
        <Section title={tx("AI service")} note={tx("Email goes straight from this Mac to that company.")}>
          <Row
            label={status.via === "codex" ? tx("ChatGPT plan") : tx("Claude plan")}
            help={status.via === "codex" ? tx("Through Codex on this Mac") : tx("Through Claude Code on this Mac")}
          >
            <button className="btn btn-secondary btn-sm" onClick={() => setRemoving(true)}>
              {tx("Disconnect")}</button>
          </Row>
          {status.models.length > 0 && (
            <Row label={tx("Model for writing")}>
              <Choice
                label={tx("Model for writing")}
                value={status.model}
                options={[
                  { value: "", label: tx("Automatic (balanced)") },
                  ...status.models.map((m) => ({ value: m, label: m[0].toUpperCase() + m.slice(1) })),
                ]}
                onChange={(m) => void chooseModel(m)}
              />
            </Row>
          )}
          {spend && spend.calls > 0 && (
            <Row label={tx("Used this month")}>
              <span className="muted tnum">{`${spend.calls} ${spend.calls === 1 ? "request" : "requests"}`}</span>
            </Row>
          )}
        </Section>
      ) : status?.has_key && !status.included && !status.using_on_device ? (
        <Section title={tx("AI service")}>
          <Row label={service?.name ?? tx("AI")} help={tx("Ending in {0}", { "0": status.ending })}>
            <button className="btn btn-secondary btn-sm" onClick={() => setRemoving(true)}>
              {tx("Remove")}</button>
          </Row>
          {status.models.length > 0 && (
            <Row label={tx("Model for writing")}>
              <Choice
                label={tx("Model for writing")}
                value={status.model}
                options={[
                  { value: "", label: tx("Automatic (balanced)") },
                  ...status.models.map((m) => ({ value: m, label: m })),
                ]}
                onChange={(m) => void chooseModel(m)}
              />
            </Row>
          )}
          {spend && spend.calls > 0 && (
            <Row label={tx("Used this month")}>
              <span className="muted tnum">
                {spend.dollars === null
                  ? `${spend.calls} ${spend.calls === 1 ? "request" : "requests"}`
                  : `$${spend.dollars < 0.01 ? "under 0.01" : spend.dollars.toFixed(2)}`}
              </span>
            </Row>
          )}
        </Section>
      ) : (
        <>
        {status && <AppleRow state={status.on_device ?? null} using={!!status.using_on_device} />}
        <PlanRows onUsed={(next) => {
          setStatus(next);
          onChanged();
        }} />
        <form onSubmit={save}>
          <div className="section-title">
            <Tier name={tx("API key")} cost={tx("Pay per use")} />
          </div>
          <div className="group">
            <div className="group-row ai-key-row">
              <span className="group-icon">
                <Icon icon={Key01Icon} size="s" />
              </span>
              <Choice
                label={tx("AI service")}
                value={serviceId}
                options={(status?.services ?? []).map((s) => ({ value: s.id, label: s.name }))}
                onChange={(id) => {
                  setPicked(id);
                  setError(null);
                }}
              />
              <PasswordInput
                aria-label={tx("AI key")}
                value={key}
                placeholder={tx("Paste your key")}
                autoComplete="off"
                onChange={(e) => setKey(e.target.value)}
              />
              <button type="submit" className="btn btn-secondary btn-sm" disabled={busy || !key.trim()}>
                {busy && <Spinner size="xs" />}
                {tx("Save")}</button>
            </div>
          </div>
          {service && (
            <p className="fine note">
              <a href={service.keys_page} target="_blank" rel="noreferrer">
                {tx("Get a {0} key", { "0": service.name })}</a>
            </p>
          )}
        </form>
        </>
      )}
      <Problem text={error} />
      <Section title={tx("Use AI for")} note={tx("AI never sends anything by itself.")}>
        <SettingToggle setting="ai_summaries" label={tx("Summaries and explanations")} />
        <SettingToggle setting="ai_replies" label={tx("Writing replies")} />
        <SettingToggle
          setting="ai_best_drafts"
          label={tx("Best quality replies")}
          help={tx("Checks each reply and improves it if needed. A little slower.")}
          disabled={settings.ai_replies !== "on"}
        />
        <SettingToggle setting="ai_translate" label={tx("Translating")} />
        <SettingToggle
          setting="ai_chat"
          label={tx("Questions about your email")}
          help={paid ? undefined : tx("Included with Mailzo Cloud")}
        />
        <SettingToggle setting="ai_rules" label={tx("Filters in plain words")} />
        <SettingToggle setting="ai_search" label={tx("Search in your own words")} />
        <SettingToggle
          setting="ai_spam"
          label={tx("Spam second opinion")}
          help={paid ? undefined : tx("Included with Mailzo Cloud")}
          disabled={!paid}
        />
      </Section>
      <Section
        title={tx("Automatic help")}
        note={tx("Sorts, summarizes and drafts replies for new mail. Important only reads just emails that need you.")}
      >
        <SettingChoice
          setting="ai_auto"
          label={tx("Read new mail by itself")}
          help={paid ? undefined : tx("Included with Mailzo Cloud")}
          disabled={!paid}
          options={[
            { value: "off", label: tx("Off") },
            { value: "important", label: tx("Important only") },
            { value: "everything", label: tx("Everything") },
          ]}
        />
        <SettingToggle
          setting="ai_draft_ahead"
          label={tx("Write replies ahead of time")}
          help={tx("Ready when you open an email that needs you. Uses your own AI.")}
        />
      </Section>
      <WritingStyle accounts={accounts} />
      <AiLimits accounts={accounts} onChanged={onAccountsChanged} />
      {removing && (
        <Confirm
          title={status?.via !== "key" ? tx("Disconnect this plan?") : tx("Remove your AI key?")}
          confirm={status?.via !== "key" ? tx("Yes, disconnect") : tx("Yes, remove")}
          keep={tx("Keep")}
          onKeep={() => setRemoving(false)}
          onConfirm={() => void remove()}
        >
          {status?.via !== "key"
            ? tx("Your plan isn't changed. Search keeps working with your exact words.")
            : tx("It will be removed from all your devices. Search keeps working with your exact words.")}
        </Confirm>
      )}
    </Page>
  );
}
