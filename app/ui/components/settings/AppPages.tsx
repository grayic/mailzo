import { FormEvent, useEffect, useRef, useState } from "react";
import { ApiError, api, canLock, native, proveItsMe, type MyTemplate } from "../../api";
import { useSettings } from "../../settings";
import type { Account, ThreadSummary } from "../../types";
import { TABS, ThreadRowBody, dayLabel, threadKey } from "../ThreadList";
import { Avatar, Confirm, Dialog, Problem, Spinner, resetPictures } from "../ui";
import { checkForUpdate, restartToUpdate, useUpdates } from "../Updates";
import { Choice, EmptyPage, Page, Row, Section, Segmented, SettingChoice, SettingToggle, Toggle } from "./controls";
import { OthersKeys } from "./Encryption";
import { LANGUAGES, chooseLanguage, chosenLanguage, tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/** Only a Mac has a menu bar and a Dock, and no Mac has a touch screen. */
const onMac = navigator.maxTouchPoints === 0;

export function GeneralPage() {
  const { settings } = useSettings();
  return (
    <Page title={tx("General")}>
      <Section>
        <SettingChoice
          setting="open_first"
          label={tx("What opens first")}
          options={[
            { value: "all", label: tx("All Inbox") },
            { value: "selected", label: tx("First inbox") },
          ]}
        />
        {settings.smart_tabs === "on" && (
          <SettingChoice
            setting="open_tab"
            label={tx("Tab that opens first")}
            menu
            options={TABS.map(([value, label]) => ({ value, label }))}
          />
        )}
        <SettingToggle
          setting="conversations"
          label={tx("Group emails into conversations")}
          help={tx("When off, each email has its own row.")}
        />
        <SettingToggle
          setting="screener"
          label={tx("Screen new senders")}
          help={tx("First-time senders wait under New senders until you let them in.")}
        />
        <SettingToggle
          setting="keyboard_keys"
          label={tx("Single-key shortcuts")}
          help={tx("Such as E to archive and J for the next email.")}
        />
      </Section>
      {onMac && (
        <Section title={tx("System")}>
          <SettingToggle
            setting="open_at_login"
            label={tx("Open at login")}
            help={tx("Starts in the background, with no window.")}
            onChanged={(on) => void native("set_open_at_login", { on })}
          />
          <SettingToggle
            setting="show_in_menu_bar"
            label={tx("Show in menu bar")}
            disabled={settings.show_in_dock !== "on"}
            onChanged={(on) => void native("set_menu_bar_icon", { on })}
          />
          <SettingToggle
            setting="show_in_dock"
            label={tx("Show in Dock")}
            help={tx("Off also hides it from ⌘-Tab.")}
            disabled={settings.show_in_menu_bar !== "on"}
            onChanged={(on) => void native("set_dock_icon", { on })}
          />
        </Section>
      )}
      <Section title={tx("Reading")}>
        <SettingChoice
          setting="mark_read"
          label={tx("Mark as read")}
          options={[
            { value: "now", label: tx("Straight away") },
            { value: "delay", label: tx("After 3 seconds") },
            { value: "manual", label: tx("Only when I choose") },
          ]}
          menu
        />
        <SettingChoice
          setting="after_action"
          label={tx("After archiving or deleting")}
          options={[
            { value: "next", label: tx("Open the next email") },
            { value: "previous", label: tx("Open the previous email") },
            { value: "list", label: tx("Go back to the list") },
          ]}
          menu
        />
        <SettingChoice
          setting="delete_key"
          label={tx("The Delete key")}
          options={[
            { value: "trash", label: tx("Moves to the bin") },
            { value: "archive", label: tx("Archives") },
          ]}
          menu
        />
        <SettingChoice
          setting="after_code_copy"
          label={tx("After copying an OTP")}
          options={[
            { value: "trash", label: tx("Moves to the bin") },
            { value: "archive", label: tx("Archives") },
            { value: "read", label: tx("Marks as read") },
            { value: "keep", label: tx("Leaves it") },
          ]}
          menu
        />
      </Section>
      {onMac && <UpdatesSection />}
    </Page>
  );
}

/** How far down the list to look for a sender with a picture, for the pictures setting to show. */
const PREVIEW_LOOK_THROUGH = 12;

const sampleRow = (id: string, from: string, subject: string, snippet: string, daysAgo: number, unread: number): ThreadSummary => ({
  needs_you: false,
  why: null,
  ai_line: null,
  due_at: null,
  thread_id: id,
  account_id: 0,
  account_email: "",
  lead_address: "",
  folder_id: 0,
  subject,
  participants: [from],
  snippet,
  last_date: Math.floor(Date.now() / 1000) - daysAgo * 86_400,
  message_count: 1,
  unread_count: unread,
  flagged: false,
  has_attachments: false,
  risk: 0,
  label: null,
  pinned: false,
  muted: false,
  snooze_until: null,
  returned_at: null,
  follow_due: false,
  follow_at: null,
});

/** Shown until there's mail of the person's own to preview. */
const SAMPLE_ROWS = [
  sampleRow("maya", tx("Maya Chen"), tx("Q4 launch plan"), tx("Looks good. I would move the press date back a week, so everyone has time to read through the plan before it goes out."), 0, 1),
  sampleRow("sam", tx("Sam Okafor"), tx("Invoice 1042 for October"), tx("Hello, the invoice is attached to our portal. Payment is due in 14 days, and I'm happy to answer any questions."), 1, 0),
];

/** The top of the real list, as wide as the list and drawn by the same code. */
function ListPreview() {
  const { settings } = useSettings();
  const [mail, setMail] = useState<ThreadSummary[]>([]);
  useEffect(() => {
    let live = true;
    void (async () => {
      const list = await api.listAll("inbox", undefined, PREVIEW_LOOK_THROUGH);
      if (!live) return;
      setMail(list.slice(0, 2));
      // One sender with a picture, if there is one, so the pictures setting shows a change.
      for (const t of list) {
        if (t.risk) continue;
        const { picture } = await api.avatar(t.lead_address);
        if (!live) return;
        if (picture) {
          if (!list.slice(0, 2).includes(t)) setMail([list[0], t]);
          return;
        }
      }
    })().catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  const rows = mail.length ? mail : SAMPLE_ROWS;
  const days = rows.reduce<{ label: string; items: ThreadSummary[] }[]>((all, t) => {
    const label = dayLabel(t.last_date);
    const last = all[all.length - 1];
    if (last?.label === label) last.items.push(t);
    else all.push({ label, items: [t] });
    return all;
  }, []);
  return (
    <div className="settings-preview settings-preview-center" inert aria-hidden="true">
      <div className="settings-preview-list">
        {days.map((day) => (
          <section key={day.label}>
            <div className="day-label">{day.label}</div>
            {day.items.map((t) => (
              <div key={threadKey(t)} className={`thread ${t.unread_count > 0 ? "thread-unread" : ""}`}>
                <ThreadRowBody
                  t={t}
                  showPictures={settings.list_pictures === "on"}
                />
              </div>
            ))}
          </section>
        ))}
      </div>
    </div>
  );
}

/** A reply in the chosen font and size, with the earlier message if that's kept. */
function WritingPreview() {
  const { settings } = useSettings();
  return (
    <div
      className="settings-preview"
      inert
      aria-hidden="true"
      data-font={settings.font}
      data-font-size={settings.font_size}
    >
      <div className="editor">
        <p>{tx("Hi Maya,")}</p>
        <p>
          <br />
        </p>
        <p>{tx("Moving the press date back a week works for me. I'll let the team know today.")}</p>
        {settings.quote === "on" && (
          <blockquote>
            <p>{tx("On Nov 1, Maya Chen wrote:")}</p>
            <p>{tx("I would move the press date back a week.")}</p>
          </blockquote>
        )}
      </div>
    </div>
  );
}

export function AppearancePage() {
  return (
    <Page title={tx("Appearance")} intro={tx("These apply to this device only.")}>
      <Section>
        <Row label={tx("Language")}>
          <Choice
            label={tx("Language")}
            value={chosenLanguage()}
            onChange={(code) => {
              chooseLanguage(code);
              // Every screen is drawn again in the new language.
              window.location.reload();
            }}
            options={[
              { value: "auto", label: tx("Follow the Mac") },
              ...LANGUAGES.map((l) => ({ value: l.code, label: l.name })),
            ]}
          />
        </Row>
        <SettingChoice
          setting="theme"
          label={tx("Theme")}
          options={[
            { value: "system", label: tx("Follow the Mac") },
            { value: "light", label: tx("Light") },
            { value: "dark", label: tx("Dark") },
          ]}
        />
        <SettingChoice
          setting="reading_size"
          label={tx("Text size")}
          help={tx("For everything in the app, including emails.")}
          options={[
            { value: "small", label: tx("Small") },
            { value: "normal", label: tx("Normal") },
            { value: "large", label: tx("Large") },
            { value: "xlarge", label: tx("Extra large") },
          ]}
        />
        <SettingChoice
          setting="delete_effect"
          label={tx("When you delete")}
          help={tx("Deleted emails and senders you leave fly off into the corner.")}
          menu
          options={[
            { value: "sound", label: tx("Fly away with sound") },
            { value: "quiet", label: tx("Fly away quietly") },
            { value: "off", label: tx("No animation") },
          ]}
        />
      </Section>
      <Section title={tx("Email list")}>
        <ListPreview />
        <LayoutPicker />
        <SettingChoice
          setting="density"
          label={tx("Spacing")}
          options={[
            { value: "comfortable", label: tx("Comfortable") },
            { value: "compact", label: tx("Compact") },
          ]}
        />
        <SettingChoice
          setting="preview_lines"
          label={tx("Lines of preview")}
          options={[
            { value: "0", label: tx("None") },
            { value: "1", label: tx("One") },
            { value: "2", label: tx("Two") },
          ]}
        />
        <SettingToggle setting="list_pictures" label={tx("Show sender pictures in the list")} />
      </Section>
      <Section>
        <SettingToggle
          setting="dock_badge"
          label={tx("Show the unread count")}
          help={tx("On the Dock and menu bar icons.")}
        />
      </Section>
    </Page>
  );
}

export function WritingPage({
  accounts,
  onSaveDefaultInbox,
}: {
  accounts: Account[];
  onSaveDefaultInbox: (accountId: number | null) => void;
}) {
  const chosen = accounts.find((a) => a.prefs.default_for_new);
  return (
    <Page title={tx("Writing")}>
      <Section>
        {accounts.length > 1 && (
          <Row label={tx("New messages come from")}>
            <Choice
              label={tx("New messages come from")}
              value={chosen ? String(chosen.id) : "selected"}
              options={[
                { value: "selected", label: tx("The selected inbox") },
                ...accounts.map((a) => ({ value: String(a.id), label: a.prefs.label || a.email })),
              ]}
              onChange={(v) => onSaveDefaultInbox(v === "selected" ? null : Number(v))}
            />
          </Row>
        )}
        <SettingChoice
          setting="undo_send"
          label={tx("Undo send")}
          help={tx("Time to take an email back after sending.")}
          options={[
            { value: "0", label: tx("Off") },
            { value: "5", label: "5s" },
            { value: "10", label: "10s" },
            { value: "20", label: "20s" },
            { value: "30", label: "30s" },
          ]}
        />
        <SettingToggle
          setting="drafts_on_server"
          label={tx("Keep drafts on your mail server too")}
          help={tx("Safe if this device is lost, and shown in your other mail apps.")}
        />
      </Section>
      <Section title={tx("Emails you write")}>
        <WritingPreview />
        <SettingChoice
          setting="font"
          label={tx("Font")}
          options={[
            { value: "system", label: tx("System") },
            { value: "arial", label: tx("Arial") },
            { value: "georgia", label: tx("Georgia") },
            { value: "times", label: tx("Times") },
            { value: "courier", label: tx("Courier") },
          ]}
          menu
        />
        <SettingChoice
          setting="font_size"
          label={tx("Size")}
          options={[
            { value: "13", label: "13" },
            { value: "14", label: "14" },
            { value: "16", label: "16" },
            { value: "18", label: "18" },
          ]}
        />
        <SettingToggle setting="spell_check" label={tx("Check spelling")} />
      </Section>
      <Section title={tx("Replies")}>
        <SettingChoice
          setting="reply_default"
          label={tx("The main Reply action")}
          help={tx("Used by ⌘R and the quick reply box.")}
          options={[
            { value: "reply", label: tx("Reply") },
            { value: "reply_all", label: tx("Reply all") },
          ]}
        />
        <SettingToggle setting="quote" label={tx("Include the earlier message in replies")} />
      </Section>
    </Page>
  );
}

const hourLabel = (h: number) => {
  const twelve = h % 12 === 0 ? 12 : h % 12;
  return `${twelve} ${h < 12 ? "AM" : "PM"}`;
};
const HOURS = Array.from({ length: 24 }, (_, h) => ({ value: String(h), label: hourLabel(h) }));

/** The value is the sound file's own name; the label is that name in the person's language. */
const ALERT_TONES = [
  { value: "Chime", label: tx("Chime") },
  { value: "Marimba", label: tx("Marimba") },
  { value: "Glass", label: tx("Glass") },
  { value: "Keys", label: tx("Keys") },
  { value: "Mac Mail", label: tx("Mac Mail") },
];

/** Each sound plays as it's chosen, so they can be compared without waiting for an email. */
function AlertTonePicker() {
  const { settings } = useSettings();
  const tone = settings.alert_tone;
  const heard = useRef(tone);
  useEffect(() => {
    if (tone && tone !== heard.current) void api.previewAlertTone(tone);
    heard.current = tone;
  }, [tone]);
  return (
    <SettingChoice
      setting="alert_tone"
      label={tx("Sound")}
      options={ALERT_TONES}
      menu
    />
  );
}

export function NotificationsPage() {
  const { settings } = useSettings();
  const on = settings.alerts === "on";
  return (
    <Page title={tx("Notifications")} intro={tx("For this device. Each inbox can have its own.")}>
      <Section>
        <SettingToggle setting="alerts" label={tx("Notify me about new email")} />
      </Section>
      {on && (
        <>
          <Section>
            <SettingChoice
              setting="alert_which"
              label={tx("Which email")}
              options={[
                { value: "important", label: tx("Only what needs me") },
                { value: "people", label: tx("People") },
                { value: "all", label: tx("All new mail") },
              ]}
            />
            <SettingToggle setting="alert_sound" label={tx("Play a sound")} />
            {settings.alert_sound === "on" && <AlertTonePicker />}
            <Row
              label={tx("Test notification")}
              help={tx("No banner? Allow Mailzo in System Settings › Notifications.")}
            >
              <button type="button" className="btn btn-secondary btn-sm" onClick={() => void api.alertTest()}>
                {tx("Send a test")}</button>
            </Row>
            <SettingToggle
              setting="alert_details"
              label={tx("Show the sender and subject")}
              help={tx("Turn off to hide them from people nearby.")}
            />
            {settings.alert_details === "on" && (
              <SettingToggle
                setting="alert_preview"
                label={tx("Show a preview of the message")}
                help={tx("A few lines of the email under its subject.")}
              />
            )}
            {settings.show_in_menu_bar !== "off" && (
              <SettingToggle
                setting="alert_menu_bar"
                label={tx("Show new mail in the menu bar")}
                help={tx("The sender and subject appear next to the Mailzo icon for a few seconds.")}
              />
            )}
          </Section>
          <Section>
            <SettingToggle setting="quiet_hours" label={tx("Quiet hours")} />
            {settings.quiet_hours === "on" && (
              <>
                <SettingChoice setting="quiet_start" label={tx("From")} options={HOURS} menu />
                <SettingChoice setting="quiet_end" label={tx("Until")} options={HOURS} menu />
              </>
            )}
          </Section>
        </>
      )}
    </Page>
  );
}

/** Words you use often, for dropping into any message. */
export function TemplatesPage() {
  const [list, setList] = useState<MyTemplate[]>([]);
  const [loaded, setLoaded] = useState(false);
  /** The template open in the window. An empty id means a new one. */
  const [open, setOpen] = useState<MyTemplate | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const load = () =>
    api
      .myTemplates()
      .then(setList)
      .catch((e) => setError(say(e)))
      .finally(() => setLoaded(true));
  useEffect(() => {
    void load();
  }, []);
  const save = async (e: FormEvent) => {
    e.preventDefault();
    // A second click while it saves would add the same template twice.
    if (!open || saving) return;
    setSaving(true);
    setError(null);
    try {
      await api.saveMyTemplate(open.id || null, open.name, open.body);
      setOpen(null);
      await load();
    } catch (err) {
      setError(say(err));
    } finally {
      setSaving(false);
    }
  };
  const remove = (t: MyTemplate) => {
    setError(null);
    void api
      .deleteMyTemplate(t.id)
      .then(load)
      .catch((e) => setError(say(e)));
  };
  return (
    <Page
      title={tx("Templates")}
      intro={tx("Words you write often, one click away.")}
      action={
        <button
          type="button"
          className="btn btn-primary btn-sm"
          onClick={() => {
            setError(null);
            setOpen({ id: "", name: "", body: "" });
          }}
        >
          {tx("New template")}</button>
      }
    >
      {list.length > 0 ? (
        <Section note={tx("Synced to your devices. Only you see them.")}>
          {list.map((t) => (
            <Row key={t.id} label={t.name} help={t.body.length > 90 ? `${t.body.slice(0, 90)}…` : t.body}>
              <div className="hstack-m">
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  aria-label={tx("Edit template {0}", { "0": t.name })}
                  onClick={() => {
                    setError(null);
                    setOpen(t);
                  }}
                >
                  {tx("Edit")}</button>
                <button
                  type="button"
                  className="btn btn-ghost btn-sm"
                  aria-label={tx("Delete template {0}", { "0": t.name })}
                  onClick={() => remove(t)}
                >
                  {tx("Delete")}</button>
              </div>
            </Row>
          ))}
        </Section>
      ) : (
        loaded && <EmptyPage title={tx("No templates yet")} text={tx("Save a reply you send often.")} />
      )}
      {!open && <Problem text={error} />}
      {open && (
        <Dialog
          title={open.id ? tx("Edit template") : tx("New template")}
          width="m"
          onClose={() => setOpen(null)}
          onSubmit={(e) => void save(e)}
          footer={
            <>
              <button type="button" className="btn btn-ghost" onClick={() => setOpen(null)}>
                {tx("Cancel")}</button>
              <button type="submit" className="btn btn-primary" disabled={saving || !open.name.trim() || !open.body.trim()}>
                {saving && <Spinner size="xs" />}
                {open.id ? tx("Save changes") : tx("Add template")}
              </button>
            </>
          }
        >
          <div className="stack">
            <label className="field">
              <span className="field-label">{tx("Name")}</span>
              <input
                autoFocus
                aria-label={tx("Template name")}
                placeholder={tx("Thank you")}
                value={open.name}
                onChange={(e) => setOpen({ ...open, name: e.target.value })}
              />
            </label>
            <label className="field">
              <span className="field-label">{tx("What it says")}</span>
              <textarea
                aria-label={tx("Template words")}
                className="input"
                placeholder={tx("Thanks for getting in touch. I'll reply soon.")}
                rows={6}
                value={open.body}
                onChange={(e) => setOpen({ ...open, body: e.target.value })}
              />
            </label>
            <Problem text={error} />
          </div>
        </Dialog>
      )}
    </Page>
  );
}

/** Locking the app behind Touch ID. Turning it on checks first that Touch ID works. */
function LockSetting() {
  const { settings, change } = useSettings();
  const [problem, setProblem] = useState<string | null>(null);
  const pick = async (value: string) => {
    setProblem(null);
    if (value !== "off" && (settings.app_lock ?? "off") === "off") {
      try {
        if (!(await proveItsMe("turn on locking for Mailzo"))) return;
      } catch (e) {
        setProblem(say(e));
        return;
      }
    }
    change("app_lock", value);
  };
  return (
    <Section
      title={tx("Lock Mailzo")}
      note={tx("Asks for Touch ID or your password before mail shows. While locked, nothing can read your mail, including AI apps. The 15-minute choice also locks on sleep and screen lock.")}
    >
      <Row label={tx("Lock with Touch ID")}>
        <Segmented
          label={tx("Lock with Touch ID")}
          value={settings.app_lock ?? "off"}
          onChange={(v) => void pick(v)}
          options={[
            { value: "off", label: tx("Off") },
            { value: "start", label: tx("When it opens") },
            { value: "away", label: tx("Also after 15 minutes away") },
          ]}
        />
      </Row>
      <Problem text={problem} />
    </Section>
  );
}

export function PrivacyPage() {
  const { settings, change, whenSaved } = useSettings();
  const [senders, setSenders] = useState<string[] | null>(null);
  const [held, setHeld] = useState<number | null>(null);
  const [trackers, setTrackers] = useState<{ company: string; seen: number }[]>([]);

  useEffect(() => {
    api
      .imageSenders()
      .then(setSenders)
      .catch(() => setSenders([]));
    api
      .privacyStats()
      .then((s) => {
        setHeld(s.images_held);
        setTrackers(s.trackers);
      })
      .catch(() => {});
  }, []);

  const remove = (address: string) => {
    setSenders((list) => list?.filter((a) => a !== address) ?? null);
    void api.setSenderImages(address, false).catch(() => {});
  };

  return (
    <Page title={tx("Privacy")}>
      {canLock && <LockSetting />}
      <Section
        title={tx("Pictures inside emails")}
        note={tx("Pictures can tell senders you opened their email. Known trackers stay blocked either way.")}
      >
        <SettingChoice
          setting="remote_images"
          label={tx("Pictures in emails")}
          options={[
            { value: "always", label: tx("Show by default") },
            { value: "ask", label: tx("Hide by default") },
          ]}
        />
        {held !== null && (
          <Row label={tx("Pictures held back")} help={tx("Opened without loading pictures.")}>
            <span className="stat tnum">
              {held} {held === 1 ? "email" : "emails"}
            </span>
          </Row>
        )}
      </Section>

      <Section title={tx("Senders whose pictures always show")}>
        {senders?.length === 0 && (
          <Row label={tx("None yet")} help={tx("Choosing \"Show pictures\" on an email adds its sender here.")} />
        )}
        {senders?.map((address) => (
          <div key={address} className="group-row" data-sender={address}>
            <Avatar name={address} email={address} size="s" />
            <span className="grow truncate-1">{address}</span>
            <button className="btn btn-ghost btn-sm" onClick={() => remove(address)}>
              {tx("Remove")}</button>
          </div>
        ))}
      </Section>

      <Section
        title={tx("Sender pictures and logos")}
        note={tx("Senders are never contacted. Gravatar gets a scrambled address, DuckDuckGo the company's website.")}
      >
        <Row label={tx("Show sender pictures and logos")}>
          <Toggle
            label={tx("Show sender pictures and logos")}
            on={settings.sender_pictures === "on"}
            onChange={(on) => {
              change("sender_pictures", on ? "on" : "off");
              void whenSaved().then(resetPictures);
            }}
          />
        </Row>
      </Section>

      {trackers.length > 0 && (
        <Section
          title={tx("Who tried to track you")}
          note={tx("Companies whose hidden pictures would have told senders you opened an email. Always blocked.")}
        >
          {trackers.map((t) => (
            <Row key={t.company} label={t.company}>
              <span className="muted tnum">
                {t.seen} {t.seen === 1 ? "email" : "emails"}
              </span>
            </Row>
          ))}
        </Section>
      )}

      <Section title={tx("What stays on your device")}>
        <Row
          label={tx("Your email")}
          help={tx("Only on your devices, fetched straight from your provider. Never through our servers.")}
        />
        <Row
          label={tx("Your passwords and inbox list")}
          help={tx("Encrypted with a key only your devices hold. We can't read it.")}
        />
        <Row
          label={tx("Finding your email settings")}
          help={tx("Only the part after the @ is looked up, in public mail directories.")}
        />
        <Row
          label={tx("What our account server knows")}
          help={tx("Your main account email and the names of your devices.")}
        />
      </Section>
      <OthersKeys />
    </Page>
  );
}

const SHORTCUTS: [string, [string, string][]][] = [
  [
    "Writing",
    [
      ["⌘N", tx("New message")],
      ["⌘R", "Reply"],
      ["⌘↩", "Send"],
      ["esc", tx("Close the message and keep it as a draft")],
    ],
  ],
  [
    tx("Moving around"),
    [
      ["⌘K", tx("Do anything by typing it")],
      ["⌘F", tx("Search all email")],
      ["⌘1", tx("All Inbox")],
      ["⌘2, ⌘3…", tx("Switch to each inbox")],
      ["⌘,", "Settings"],
      ["⌘\\", tx("Collapse or expand the sidebar")],
    ],
  ],
  [
    tx("Email list"),
    [
      ["⌘A", tx("Select everything in the list")],
      ["⌘ click", tx("Select one more")],
      ["⇧ click", tx("Select a range")],
      ["esc", tx("Clear the selection")],
      [tx("Delete"), tx("Move to the bin or archive, as chosen in General")],
    ],
  ],
  [
    tx("Single keys"),
    [
      ["J / K", tx("Next or previous email")],
      ["E", "Archive"],
      ["#", "Delete"],
      ["D", tx("Done: out of Needs you")],
      ["S", tx("Star or remove the star")],
      ["U", tx("Mark read or unread")],
      ["H", tx("Snooze until tomorrow morning")],
      ["P", tx("Pin or unpin")],
      ["M", "Mute"],
      ["R / A / F", tx("Reply, reply all or forward")],
      ["C", tx("New message")],
      ["/", "Search"],
    ],
  ],
];

export function ShortcutsPage() {
  return (
    <Page title={tx("Keyboard shortcuts")}>
      {SHORTCUTS.map(([title, list]) => (
        <Section key={title} title={title}>
          {list.map(([keys, what]) => (
            <Row key={keys} label={what}>
              <span className="shortcut">{keys}</span>
            </Row>
          ))}
        </Section>
      ))}
    </Page>
  );
}

export function AdvancedPage({ onCleared }: { onCleared: () => void }) {
  const [confirming, setConfirming] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const clear = async () => {
    setConfirming(false);
    setError(null);
    try {
      await api.clearSavedMail();
      setDone(true);
      onCleared();
    } catch (e) {
      setError(say(e));
    }
  };

  return (
    <Page title={tx("Advanced")}>
      <Section>
        <Row
          label={tx("Clear saved mail on this device")}
          help={
            done
              ? tx("Cleared. Your mail is downloading again.")
              : tx("Downloads this Mac's copy again. Your mail and drafts are kept.")
          }
        >
          <button className="btn btn-secondary btn-sm" onClick={() => setConfirming(true)}>
            {tx("Clear and download again")}</button>
        </Row>
      </Section>
      <Problem text={error} />
      {confirming && (
        <Confirm
          title={tx("Clear saved mail on this device?")}
          confirm={tx("Yes, clear it")}
          keep={tx("Keep")}
          onKeep={() => setConfirming(false)}
          onConfirm={() => void clear()}
        >
          {tx("Mail downloads again, which can take a while. Nothing is deleted from your provider.")}</Confirm>
      )}
    </Page>
  );
}

const LAYOUTS = [
  { value: "side", label: tx("Email beside the list") },
  { value: "full", label: tx("Full width, like Gmail") },
] as const;

/** The two layouts drawn small, so the choice is easy to picture. */
function LayoutPicker() {
  const { settings, change } = useSettings();
  const chosen = settings.layout === "full" ? "full" : "side";
  return (
    <Row label={tx("Layout")} stacked>
      <div role="radiogroup" aria-label={tx("Layout")} className="layout-picker">
        {LAYOUTS.map((l) => (
          <button
            key={l.value}
            type="button"
            role="radio"
            aria-checked={chosen === l.value}
            className="layout-option"
            onClick={() => change("layout", l.value)}
          >
            <span className="layout-sketch" data-layout={l.value} aria-hidden="true">
              <span className="sketch-side" />
              <span className="sketch-list">
                {[0, 1, 2, 3, 4].map((i) => (
                  <span key={i} className="sketch-row" />
                ))}
              </span>
              <span className="sketch-reader">
                <span className="sketch-line sketch-title" />
                <span className="sketch-line" />
                <span className="sketch-line" />
                <span className="sketch-line sketch-short" />
              </span>
            </span>
            <span className="layout-label">{l.label}</span>
          </button>
        ))}
      </div>
    </Row>
  );
}


/** The version on this Mac, and new versions coming from inside the app. */
function UpdatesSection() {
  const { settings, change } = useSettings();
  const { version, checking, ready, restarting, said, failed } = useUpdates();
  const channel = settings.update_channel ?? "stable";
  return (
    <Section title={tx("Updates")}>
      <Row
        label={version ? `Mailzo ${version}` : tx("Mailzo")}
        help={
          failed ??
          (ready
            ? tx("Mailzo {0} is downloaded. Restart to use it. Your drafts are kept.", { "0": ready.version })
            : (said ?? tx("Updates wait until you restart.")))
        }
      >
        {ready ? (
          <button className="btn btn-primary btn-sm" disabled={restarting} onClick={() => void restartToUpdate(channel)}>
            {restarting && <Spinner size="xs" />}
            {failed ? tx("Try again") : tx("Restart to update")}
          </button>
        ) : (
          <button className="btn btn-secondary btn-sm" disabled={checking} onClick={() => void checkForUpdate(channel, true)}>
            {checking && <Spinner size="xs" />}
            {checking ? "Checking…" : tx("Check for updates")}
          </button>
        )}
      </Row>
      <SettingToggle setting="auto_update" label={tx("Download updates automatically")} />
      <Row
        label={tx("Get early versions")}
        help={tx("New versions a few days early.")}
      >
        <Toggle
          label={tx("Get early versions")}
          on={channel === "beta"}
          onChange={(on) => change("update_channel", on ? "beta" : "stable")}
        />
      </Row>
    </Section>
  );
}
