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
    <Page title={tx("Spam and sorting")} intro={tx("Worked out on your device. Nothing is deleted.")}>
      <Section
        title={tx("Spam")}
        note={
          moving
            ? tx("Spam moves to your Spam folder, and each email says why. \"Not spam\" brings it back.")
            : tx("Trial mode: nothing moved yet. Check the list, then turn on moving.")
        }
      >
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
          label={tx("Spam second opinion")}
          help={paid ? tx("AI takes a look at emails the filter is unsure about.") : tx("Included with Mailzo Cloud")}
          disabled={!paid}
        />
      </Section>

      <Section title={moving ? tx("Waiting to move") : tx("What it would move")}>
        {report && waiting.length === 0 && (
          <Row label={tx("Nothing")} help={tx("Nothing in your inbox looks like spam right now.")} />
        )}
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

      {examples && (
        <Section title={tx("What it has learned")}>
          <Row
            label={tx("{0} spam and {1} good examples", { "0": examples.spam, "1": examples.good })}
            help={
              examples.spam < examples.needed || examples.good < examples.needed
                ? tx("The learning part starts once it has {0} of each. It learns from your Spam folder, your \"Spam\" and \"Not spam\" choices, and people you write to.", { "0": examples.needed })
                : tx("It learns from your Spam folder, your \"Spam\" and \"Not spam\" choices, and people you write to.")
            }
          />
        </Section>
      )}

      <Section title={tx("Sorting")} note={tx("Tabs only change the view here. Nothing moves on your server.")}>
        <SettingToggle
          setting="smart_tabs"
          label={tx("Sort the inbox into tabs")}
          help={tx("Needs you, Good to know, The rest, and All.")}
        />
      </Section>

      <Section title={tx("Blocked senders")}>
        {lists?.blocked.length === 0 && <Row label={tx("None")} help={tx("Right-click an email to block its sender.")} />}
        {lists?.blocked.map((address) => (
          <Row key={address} label={address}>
            <button className="btn btn-ghost btn-sm" onClick={() => void unlist(address, "blocked")}>
              {tx("Unblock")}</button>
          </Row>
        ))}
      </Section>

      <Section title={tx("Trusted senders")}>
        {lists?.trusted.length === 0 && <Row label={tx("None")} help={tx("Choosing \"Not spam\" on an email adds its sender here.")} />}
        {lists?.trusted.map((address) => (
          <Row key={address} label={address}>
            <button className="btn btn-ghost btn-sm" onClick={() => void unlist(address, "trusted")}>
              {tx("Remove")}</button>
          </Row>
        ))}
      </Section>

      <Section title={tx("Always on a tab")}>
        {lists && Object.keys(lists.sorted).length === 0 && (
          <Row label={tx("None")} help={tx("Right-click an email to always put its sender on a tab.")} />
        )}
        {lists &&
          Object.entries(lists.sorted).map(([address, tab]) => (
            <Row key={address} label={address} help={TAB_NAMES[tab] ?? tab}>
              <button className="btn btn-ghost btn-sm" onClick={() => void unlist(address, "sorted")}>
                {tx("Remove")}</button>
            </Row>
          ))}
      </Section>
      <Problem text={error ?? (report?.ai_problem ? tx("The AI didn't help this time: {0}", { "0": report.ai_problem }) : null)} />
    </Page>
  );
}
