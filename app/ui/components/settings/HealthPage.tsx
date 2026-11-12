import { useEffect, useState } from "react";
import { ApiError, api, type MailHealth } from "../../api";
import { fileSize, timeAgo } from "../../format";
import { ArrowDown01Icon, ArrowRight01Icon } from "../../icons";
import { Icon, Problem } from "../ui";
import { Page, Row, Section, SettingChoice } from "./controls";
import { language, tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

function when(unix: number | null) {
  if (!unix) return tx("Not yet");
  const ago = timeAgo(unix);
  return ago[0].toUpperCase() + ago.slice(1);
}

type Inbox = MailHealth["inboxes"][number];

/** One inbox as one line: how it's doing at a glance, with its folders a click away. */
function InboxRow({ inbox }: { inbox: Inbox }) {
  const stuck = inbox.folders.some((f) => f.problem);
  const [open, setOpen] = useState(stuck);
  const toCome = inbox.folders.reduce(
    (sum, f) => sum + Math.max(0, (f.server_total ?? f.saved) - f.saved) + f.bodies_to_come,
    0,
  );
  const status = inbox.problem
    ? tx("Last try failed: {0}", { "0": inbox.problem })
    : toCome > 0
      ? tx("{0} still to download.", { "0": toCome.toLocaleString(language) })
      : tx("Everything's here.");
  return (
    <div className="health-inbox">
      <button type="button" className="group-row health-inbox-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <div className="grow">
          <div className="row-label">{inbox.email}</div>
          <div className={`row-help ${inbox.problem || stuck ? "health-bad" : ""}`}>{status}</div>
        </div>
        <span className={inbox.problem ? "health-bad" : "muted"}>{when(inbox.last_ok)}</span>
        <Icon icon={open ? ArrowDown01Icon : ArrowRight01Icon} size="xs" />
      </button>
      {open && (
        <div className="health-folders">
          {inbox.folders.map((f) => (
            <div key={f.id} className="health-folder">
              <div className="grow">
                {f.name}
                {f.problem && <div className="row-help health-bad">{f.problem}</div>}
              </div>
              <span className={f.problem ? "health-bad tnum" : "muted tnum"}>
                {f.server_total !== null && f.saved < f.server_total
                  ? tx("{0} of {1}", {
                      "0": f.saved.toLocaleString(language),
                      "1": f.server_total.toLocaleString(language),
                    })
                  : f.saved.toLocaleString(language)}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** What's on this device, how up to date it is, and anything stuck. */
export function HealthPage() {
  const [health, setHealth] = useState<MailHealth | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const load = () =>
      api
        .mailHealth()
        .then(setHealth)
        .catch((e) => setError(say(e)));
    void load();
    const timer = setInterval(load, 5000);
    return () => clearInterval(timer);
  }, []);

  return (
    <Page
      title={tx("Mail on this device")}
      intro={tx("Your mail downloads here, so you can read and search it offline.")}
    >
      <Section>
        <SettingChoice
          setting="keep_mail"
          label={tx("Keep whole emails from")}
          help={tx("Older mail keeps sender and subject, and downloads when opened.")}
          options={[
            { value: "all", label: tx("All time") },
            { value: "year", label: tx("The past year") },
            { value: "quarter", label: tx("The past 3 months") },
          ]}
          menu
        />
        {health && <Row label={tx("Room used on this device")}>{fileSize(health.bytes)}</Row>}
      </Section>
      <Problem text={error} />
      {health && health.inboxes.length > 0 && (
        <Section title={tx("Inboxes")}>
          {health.inboxes.map((inbox) => (
            <InboxRow key={inbox.account_id} inbox={inbox} />
          ))}
        </Section>
      )}
    </Page>
  );
}
