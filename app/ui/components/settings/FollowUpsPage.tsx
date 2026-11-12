import { useEffect, useState } from "react";
import { ApiError, api, type FollowUpRun, type FollowUps, type FollowUpStep } from "../../api";
import { Confirm, Problem } from "../ui";
import { EmptyPage, Page, Row, Section } from "./controls";
import { language, tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const day = (unix: number) => new Date(unix * 1000).toLocaleDateString(language, { month: "short", day: "numeric" });

/** When each follow-up goes, in words. */
export function scheduleText(steps: FollowUpStep[]): string {
  let total = 0;
  const days = steps.map((s) => {
    total += s.days;
    return tx("day {n}", { n: total });
  });
  return new Intl.ListFormat(language, { type: "unit", style: "short" }).format(days);
}

/** Follow-ups on their way now, and any saved for next time. They're set up from a conversation. */
export function FollowUpsPage() {
  const [runs, setRuns] = useState<FollowUpRun[]>([]);
  const [saved, setSaved] = useState<FollowUps[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [deleting, setDeleting] = useState<FollowUps | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    Promise.all([api.sequenceRuns(), api.sequencesList()])
      .then(([r, s]) => {
        setRuns(r.filter((x) => x.state === "running"));
        setSaved(s);
      })
      .catch((e) => setError(say(e)))
      .finally(() => setLoaded(true));
  useEffect(() => {
    void load();
  }, []);

  const stop = (run: FollowUpRun) =>
    void api
      .sequenceStop(run.id)
      .then(load)
      .catch((e) => setError(say(e)));

  return (
    <Page title={tx("Follow-ups")} intro={tx("Start them from a conversation you sent, with Follow up.")}>
      {runs.length > 0 && (
        <Section title={tx("On their way")}>
          {runs.map((run) => (
            <Row
              key={run.id}
              label={run.subject || tx("(no subject)")}
              help={
                run.next_at
                  ? tx("{0} of {1} sent, next on {2}", { "0": run.sent, "1": run.steps, "2": day(run.next_at) })
                  : tx("{0} of {1} sent", { "0": run.sent, "1": run.steps })
              }
            >
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => stop(run)}>
                {tx("Stop")}
              </button>
            </Row>
          ))}
        </Section>
      )}
      {saved.length > 0 && (
        <Section title={tx("Saved for next time")}>
          {saved.map((f) => (
            <Row
              key={f.id}
              label={f.name}
              help={
                f.steps.length === 1
                  ? tx("1 follow-up, on {days}", { days: scheduleText(f.steps) })
                  : tx("{n} follow-ups, on {days}", { n: f.steps.length, days: scheduleText(f.steps) })
              }
            >
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                aria-label={tx("Delete {0}", { "0": f.name })}
                onClick={() => setDeleting(f)}
              >
                {tx("Delete")}
              </button>
            </Row>
          ))}
        </Section>
      )}
      {loaded && runs.length === 0 && saved.length === 0 && (
        <EmptyPage
          title={tx("No follow-ups on their way")}
          text={tx("Open an email you sent and choose Follow up.")}
        />
      )}
      <Problem text={error} />
      {deleting && (
        <Confirm
          title={tx("Delete {0}?", { "0": deleting.name })}
          confirm={tx("Delete")}
          keep={tx("Keep")}
          danger
          onKeep={() => setDeleting(null)}
          onConfirm={() => {
            const gone = deleting;
            setDeleting(null);
            void api
              .sequenceDelete(gone.id)
              .then(load)
              .catch((e) => setError(say(e)));
          }}
        >
          {tx("Any of these still waiting to go are stopped. Emails already sent stay sent.")}
        </Confirm>
      )}
    </Page>
  );
}
