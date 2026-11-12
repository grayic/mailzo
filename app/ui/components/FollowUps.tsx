import { useEffect, useState } from "react";
import { ApiError, api, type FollowUpRun, type FollowUps, type FollowUpStep } from "../api";
import { InformationCircleIcon, SentIcon } from "../icons";
import { Dialog, Icon, Menu, MenuItem, Problem, Spinner } from "./ui";
import { Toggle } from "./settings/controls";
import { language, tx } from "../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const day = (unix: number) => new Date(unix * 1000).toLocaleDateString(language, { month: "short", day: "numeric" });

/** The follow-ups on one conversation, and a way to stop them. */
export function useFollowUps(accountId: number, threadId: string, tick: number) {
  const [runs, setRuns] = useState<FollowUpRun[]>([]);
  const [again, setAgain] = useState(0);
  useEffect(() => {
    if (!threadId) return;
    let live = true;
    api
      .sequenceRuns(accountId, threadId)
      .then((r) => live && setRuns(r))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [accountId, threadId, tick, again]);
  return { runs, reload: () => setAgain((n) => n + 1) };
}

/** Shows follow-ups that are on their way, or why they stopped going. */
export function FollowUpBar({ run, onChanged }: { run: FollowUpRun; onChanged: () => void }) {
  const [problem, setProblem] = useState<string | null>(null);
  const words =
    run.state === "running"
      ? (run.next_at
        ? tx("{0}: {1} of {2} sent, next on {3}. Stops if they reply.", { "0": run.sequence_name, "1": run.sent, "2": run.steps, "3": day(run.next_at) })
        : tx("{0}: {1} of {2} sent. Stops if they reply.", { "0": run.sequence_name, "1": run.sent, "2": run.steps }))
      : `${run.sequence_name} stopped: ${run.problem ?? "it couldn't send"}`;
  return (
    <>
      <div role="status" className="follow-bar">
        <Icon icon={SentIcon} size="s" />
        <span>{words}</span>
        {run.state === "running" && (
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() =>
              void api
                .sequenceStop(run.id)
                .then(onChanged)
                .catch((e) => setProblem(say(e)))
            }
          >
            {tx("Stop")}</button>
        )}
      </div>
      <Problem text={problem} />
    </>
  );
}

type StepDraft = { when: "days" | "date"; days: number; date: string; ai: boolean; body: string };

const MOST_FOLLOW_UPS = 10;
const QUICK_COUNTS = [1, 3, 5];
/** Gaps that feel natural for the first few nudges; later ones wait a week. */
const GAPS = [3, 4, 7];
const gap = (i: number) => GAPS[i] ?? 7;
const blank = (i: number): StepDraft => ({ when: "days", days: gap(i), date: "", ai: true, body: "Hi {first_name},\n\n" });

/** A date-and-time box's value, in the person's own time. */
const localInput = (unix: number) => {
  const d = new Date(unix * 1000);
  d.setMinutes(d.getMinutes() - d.getTimezoneOffset());
  return d.toISOString().slice(0, 16);
};

/** Sets up follow-ups for an email the person sent, right from the conversation. */
export function StartFollowUps({
  messageId,
  onClose,
  onStarted,
}: {
  messageId: number;
  onClose: () => void;
  onStarted: () => void;
}) {
  const [steps, setSteps] = useState<StepDraft[]>(() => [0, 1, 2].map(blank));
  const [saved, setSaved] = useState<FollowUps[]>([]);
  const [keep, setKeep] = useState(false);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    api
      .sequencesList()
      .then(setSaved)
      .catch(() => {});
  }, []);

  const setCount = (n: number) => {
    const count = Math.max(1, Math.min(MOST_FOLLOW_UPS, n));
    setSteps((list) => Array.from({ length: count }, (_, i) => list[i] ?? blank(i)));
  };
  const setStep = (i: number, change: Partial<StepDraft>) =>
    setSteps((list) => list.map((s, j) => (j === i ? { ...s, ...change } : s)));
  const applySaved = (f: FollowUps) =>
    setSteps(f.steps.map((s) => ({ when: "days", days: s.days, date: "", ai: s.ai && !s.body.trim(), body: s.body })));

  const hasDate = steps.some((s) => s.when === "date");
  const ready = steps.every(
    (s) => (s.when === "days" ? s.days >= 1 && s.days <= 90 : s.date !== "") && (s.ai || s.body.trim() !== ""),
  );

  const start = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!ready || busy) return;
    setProblem(null);
    setBusy(true);
    const built: FollowUpStep[] = steps.map((s) => ({
      days: s.when === "days" ? s.days : 0,
      body: s.ai ? "" : s.body,
      ai: s.ai,
      ...(s.when === "date" ? { at: Math.floor(new Date(s.date).getTime() / 1000) } : {}),
    }));
    try {
      if (keep && !hasDate && name.trim()) {
        await api.sequenceSave({ id: 0, name: name.trim(), steps: built });
      }
      await api.sequenceStartSteps(messageId, built);
      onStarted();
      onClose();
    } catch (err) {
      setProblem(say(err));
    } finally {
      setBusy(false);
    }
  };

  const soonest = localInput(Math.floor(Date.now() / 1000) + 3600);
  return (
    <Dialog
      title={tx("Follow up automatically")}
      width="l"
      onClose={onClose}
      onSubmit={(e) => void start(e)}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            {tx("Cancel")}
          </button>
          <button type="submit" className="btn btn-primary" disabled={!ready || busy || (keep && !hasDate && !name.trim())}>
            {busy ? <Spinner size="xs" /> : null}
            {steps.length === 1 ? tx("Start 1 follow-up") : tx("Start {n} follow-ups", { n: steps.length })}
          </button>
        </>
      }
    >
      <div className="stack">
        <div className="follow-stops" role="note">
          <Icon icon={InformationCircleIcon} size="s" />
          <span>{tx("The whole sequence stops as soon as anyone replies in this conversation.")}</span>
        </div>

        <div className="follow-count">
          <span className="field-label grow">{tx("How many follow-ups until they reply?")}</span>
          <div role="radiogroup" aria-label={tx("How many follow-ups")} className="segmented">
            {QUICK_COUNTS.map((n) => (
              <button
                key={n}
                type="button"
                role="radio"
                aria-checked={steps.length === n}
                className="segment tnum"
                onClick={() => setCount(n)}
              >
                {n}
              </button>
            ))}
          </div>
          <input
            type="number"
            min={1}
            max={MOST_FOLLOW_UPS}
            className="input tnum follow-count-input"
            aria-label={tx("Number of follow-ups, up to {n}", { n: MOST_FOLLOW_UPS })}
            value={steps.length}
            onChange={(e) => setCount(Number(e.target.value) || 1)}
          />
          {saved.length > 0 && (
            <Menu
              label={tx("Use saved follow-ups")}
              align="end"
              trigger={(props) => (
                <button {...props} type="button" className="btn btn-ghost btn-sm">
                  {tx("Use saved")}
                </button>
              )}
            >
              {(close) =>
                saved.map((f) => (
                  <MenuItem
                    key={f.id}
                    onClick={() => {
                      close();
                      applySaved(f);
                    }}
                  >
                    {f.name}
                  </MenuItem>
                ))
              }
            </Menu>
          )}
        </div>

        {steps.map((step, i) => (
          <fieldset key={i} className="follow-step">
            <div className="follow-step-head">
              <legend className="field-label">{tx("Follow-up {n}", { n: i + 1 })}</legend>
              <span className="grow" />
              <div role="radiogroup" aria-label={tx("When follow-up {n} goes", { n: i + 1 })} className="segmented">
                <button
                  type="button"
                  role="radio"
                  aria-checked={step.when === "days"}
                  className="segment"
                  onClick={() => setStep(i, { when: "days" })}
                >
                  {tx("After days")}
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={step.when === "date"}
                  className="segment"
                  onClick={() => setStep(i, { when: "date", date: step.date || soonest })}
                >
                  {tx("On a date")}
                </button>
              </div>
            </div>
            {step.when === "days" ? (
              <label className="follow-days">
                <span className="fine">{tx("Wait")}</span>
                <input
                  type="text"
                  inputMode="numeric"
                  className="input tnum"
                  aria-label={tx("Days to wait before follow-up {0}", { "0": i + 1 })}
                  value={String(step.days || "")}
                  onChange={(e) => setStep(i, { days: Number(e.target.value.replace(/\D/g, "")) || 0 })}
                />
                <span className="fine">{i === 0 ? tx("days after your email") : tx("days after the one before")}</span>
              </label>
            ) : (
              <input
                type="datetime-local"
                className="input follow-date"
                aria-label={tx("When follow-up {n} goes", { n: i + 1 })}
                min={soonest}
                value={step.date}
                onChange={(e) => setStep(i, { date: e.target.value })}
              />
            )}
            <div className="follow-step-ai">
              <span className="grow">
                <span className="row-label">{tx("Let AI write it")}</span>
                <span className="row-help">
                  {step.ai ? tx("Written from the conversation just before it goes.") : tx("{token} becomes their first name.", { token: "{first_name}" })}
                </span>
              </span>
              <Toggle
                label={tx("Let AI write follow-up {n}", { n: i + 1 })}
                on={step.ai}
                onChange={(on) => setStep(i, { ai: on })}
              />
            </div>
            {!step.ai && (
              <textarea
                className="input"
                aria-label={tx("What follow-up {0} says", { "0": i + 1 })}
                rows={3}
                value={step.body}
                onChange={(e) => setStep(i, { body: e.target.value })}
              />
            )}
          </fieldset>
        ))}

        {!hasDate && (
          <label className="check-row">
            <input type="checkbox" className="check" checked={keep} onChange={(e) => setKeep(e.target.checked)} />
            <span>{tx("Save for next time")}</span>
          </label>
        )}
        {keep && !hasDate && (
          <input
            className="input"
            aria-label={tx("Name for these follow-ups")}
            placeholder={tx("Chase a proposal")}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        )}
        <Problem text={problem} />
      </div>
    </Dialog>
  );
}
