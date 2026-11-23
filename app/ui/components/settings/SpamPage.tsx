import { useCallback, useEffect, useState } from "react";
import { ApiError, api, type SpamLists, type SpamReport } from "../../api";
import { useSettings } from "../../settings";
import { Avatar, Problem } from "../ui";
import { Page, Row, Section, SettingToggle, Segmented } from "./controls";
import { tx } from "../../i18n";
import { usePaid } from "../../paid";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const TAB_NAMES: Record<string, string> = {
  general: tx("Good to know"),
  newsletters: tx("The rest"),
  promotions: tx("The rest"),
};

export function SpamPage({ onChanged }: { onChanged: () => void }) {
  const { settings, change, whenSaved } = useSettings();
  // The AI second opinion comes with Mailzo Cloud.
  const paid = usePaid();
  const [report, setReport] = useState<SpamReport | null>(null);
  const [lists, setLists] = useState<SpamLists | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    api
      .spamReport()
      .then(setReport)
      .catch((e) => setError(say(e)));
    api
      .spamLists()
      .then(setLists)
      .catch((e) => setError(say(e)));
  }, []);

  useEffect(load, [load]);

  const notSpam = async (row: SpamReport["would_move"][number]) => {
    setError(null);
    try {
      await api.spamClear([{ account_id: row.account_id, thread_id: row.thread_id, folder_id: row.folder_id }]);
      load();
      onChanged();
    } catch (e) {
      setError(say(e));
    }
  };

  const unlist = async (address: string, kind: "blocked" | "trusted" | "sorted") => {
    setError(null);
    try {
      setLists(await api.spamUnlist(address, kind));
      onChanged();
    } catch (e) {
      setError(say(e));
    }
  };

  const moving = settings.spam_mode === "move";
  const waiting = report?.would_move ?? [];
  const examples = report?.examples;

  return (
    <Page title={tx("Spam")}>
      <Section>
        <Row label={tx("When an email looks like spam")}>
          <Segmented
            label={tx("When an email looks like spam")}
            value={settings.spam_mode ?? "trial"}
            options={[
              { value: "trial", label: tx("Only warn me") },
              { value: "move", label: tx("Move to Spam") },
            ]}
            onChange={(v) => {
              change("spam_mode", v);
              void whenSaved().then(onChanged);
            }}
          />
        </Row>
        <SettingToggle
          setting="ai_spam"
          label={tx("AI second opinion")}
          help={paid ? undefined : tx("Included with Mailzo Cloud")}
          disabled={!paid}
        />
        <SettingToggle setting="smart_tabs" label={tx("Sort the inbox into tabs")} />
      </Section>

      <Section title={moving ? tx("Waiting to move") : tx("What it would move")}>
        {report && waiting.length === 0 && <Row label={tx("Nothing right now")} />}
        {waiting.map((row) => (
          <div key={row.thread_id} className="group-row" data-would-move={row.subject}>
            <Avatar name={row.from_name || row.from_address} size="s" />
            <div className="grow">
              <div className="row-label truncate-1">{row.subject || tx("(no subject)")}</div>
              <div className="row-help">{row.from_address}</div>
              {row.reasons.map((reason) => (
                <div key={reason} className="row-help">
                  {reason}
                </div>
              ))}
            </div>
            <button className="btn btn-secondary btn-sm" onClick={() => void notSpam(row)}>
              {tx("Not spam")}</button>
          </div>
        ))}
      </Section>

      <Section title={tx("Trusted senders")}>
        {lists?.trusted.length === 0 && <Row label={tx("None")} />}
        {lists?.trusted.map((address) => (
          <Row key={address} label={address}>
            <button className="btn btn-ghost btn-sm" onClick={() => void unlist(address, "trusted")}>
              {tx("Remove")}</button>
          </Row>
        ))}
      </Section>

      <Section title={tx("Always on a tab")}>
        {lists && Object.keys(lists.sorted).length === 0 && <Row label={tx("None")} />}
        {lists &&
          Object.entries(lists.sorted).map(([address, tab]) => (
            <Row key={address} label={address} help={TAB_NAMES[tab] ?? tab}>
              <button className="btn btn-ghost btn-sm" onClick={() => void unlist(address, "sorted")}>
                {tx("Remove")}</button>
            </Row>
          ))}
      </Section>
      {examples && (
        <p className="fine note">
          {tx("Learned from {0} spam and {1} good emails", {
            "0": examples.spam.toLocaleString(),
            "1": examples.good.toLocaleString(),
          })}
        </p>
      )}
      <Problem text={error ?? (report?.ai_problem ? tx("The AI didn't help this time: {0}", { "0": report.ai_problem }) : null)} />
    </Page>
  );
}

/** Senders blocked by hand: a list, and a box to add one by typing it. */
export function BlockedPage({ onChanged }: { onChanged: () => void }) {
  const [blocked, setBlocked] = useState<string[] | null>(null);
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .spamLists()
      .then((l) => setBlocked(l.blocked))
      .catch((e) => setError(say(e)));
  }, []);

  const block = async () => {
    if (!address.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      setBlocked((await api.spamBlock(address)).blocked);
      setAddress("");
      onChanged();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const unblock = async (who: string) => {
    setError(null);
    try {
      setBlocked((await api.spamUnlist(who, "blocked")).blocked);
      onChanged();
    } catch (e) {
      setError(say(e));
    }
  };

  return (
    <Page title={tx("Blocked")}>
      <form
        className="hstack"
        onSubmit={(e) => {
          e.preventDefault();
          void block();
        }}
      >
        <input
          className="input grow"
          type="email"
          aria-label={tx("Address to block")}
          placeholder="name@company.com"
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
        <button className="btn btn-secondary" disabled={busy}>
          {tx("Block")}</button>
      </form>
      <Section
        title={tx("Blocked senders")}
        note={tx("Their emails go straight to Spam.")}
      >
        {blocked?.length === 0 && <Row label={tx("None")} />}
        {blocked?.map((who) => (
          <Row key={who} label={who}>
            <button className="btn btn-ghost btn-sm" onClick={() => void unblock(who)}>
              {tx("Unblock")}</button>
          </Row>
        ))}
      </Section>
      <Problem text={error} />
    </Page>
  );
}
