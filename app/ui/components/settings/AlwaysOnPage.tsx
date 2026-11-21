import { useCallback, useEffect, useState, type ReactNode } from "react";
import { ApiError, api, copyText, onTheWeb, type HelperStatus, type PairStatus, type PairedDevice } from "../../api";
import { CloudIcon, CloudServerIcon, Copy01Icon, LaptopIcon, Tick02Icon } from "../../icons";
import { Confirm, Icon, Problem, Spinner } from "../ui";
import { Page, Row, Section } from "./controls";
import { tx } from "../../i18n";

/** Mailzo Cloud isn't running yet: its button says so until it launches. */
const CLOUD_LIVE = false;

/** Puts a piece of the page, such as bold words, where `{0}` is in a translated sentence. */
const fill = (text: string, part: ReactNode) => {
  const [before, after = ""] = text.split("{0}");
  return (
    <>
      {before}
      {part}
      {after}
    </>
  );
};

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

const when = (unix: number) =>
  new Date(unix * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/** Settings › Connect (the always-on server). In the Mac app: pair with Mailzo Cloud or a server of your own.
 * On the web: pair a Mac with this server. */
export function AlwaysOnPage({ signedIn }: { signedIn: boolean }) {
  return onTheWeb ? <ConnectAMac /> : <OnTheMac signedIn={signedIn} />;
}

function OnTheMac({ signedIn }: { signedIn: boolean }) {
  const [status, setStatus] = useState<HelperStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [own, setOwn] = useState(false);
  const [confirmOff, setConfirmOff] = useState(false);

  const load = useCallback(() => {
    api.helperStatus().then(setStatus, (e) => setError(say(e)));
  }, []);
  useEffect(load, [load]);

  const act = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
      load();
    }
  };

  if (!status) return <Page title={tx("Connect")}>{error ? <Problem text={error} /> : <Spinner />}</Page>;

  if (status.connected) {
    return (
      <Page
        title={tx("Connect")}
        intro={tx("Your filters, scheduled emails, snoozes and reminders keep working while this Mac is off.")}
      >
        <Section>
          <Row
            icon={status.kind === "cloud" ? CloudIcon : CloudServerIcon}
            label={status.kind === "cloud" ? tx("Mailzo Cloud") : tx("Your own server")}
            help={status.url ?? undefined}
          />
          <Row
            label={tx("Last in step")}
            help={status.synced_at ? when(status.synced_at) : tx("Not yet")}
          >
            <button className="btn btn-secondary" disabled={busy} onClick={() => void act(api.helperSyncNow)}>
              {tx("Sync now")}
            </button>
          </Row>
          <Row
            label={tx("Scheduled emails it's sending")}
            help={tx("Change one here and it's taken back from the server first, so it never goes twice.")}
          >
            <span className="strong">{status.scheduled_on_server}</span>
          </Row>
        </Section>
        {(status.problem || error) && <Problem text={error ?? status.problem} />}
        <button className="btn btn-secondary btn-block" disabled={busy} onClick={() => setConfirmOff(true)}>
          {tx("Disconnect")}
        </button>
        {confirmOff && (
          <Confirm
            title={tx("Disconnect this Mac?")}
            confirm={tx("Disconnect")}
            keep={tx("Keep it connected")}
            onKeep={() => setConfirmOff(false)}
            onConfirm={() => {
              setConfirmOff(false);
              void act(async () => {
                try {
                  await api.helperDisconnect(false);
                } catch (e) {
                  // The server can't be reached: disconnecting anyway leaves what it holds
                  // unscheduled here, never sent from here by itself.
                  if (e instanceof ApiError && e.code === "helper_offline") {
                    await api.helperDisconnect(true);
                  } else throw e;
                }
              });
            }}
          >
            {tx("Scheduled emails it holds come back to this Mac first, and go from here while it's open.")}
          </Confirm>
        )}
      </Page>
    );
  }

  if (own || status.pending) return <PairOwnServer onDone={() => { setOwn(false); load(); }} />;

  return (
    <Page
      title={tx("Connect")}
      intro={tx("Keep your filters, scheduled emails, snoozes and reminders working while this Mac is off.")}
    >
      <Section>
        <Row
          icon={CloudIcon}
          label={tx("Mailzo Cloud")}
          help={tx("We run it for you. Also lets you read your mail on the web, from any phone or computer.")}
        >
          {CLOUD_LIVE ? (
            <button className="btn btn-primary" disabled={busy || !signedIn} onClick={() => void act(api.helperCloudOn)}>
              {tx("Turn on Mailzo Cloud")}
            </button>
          ) : (
            <button className="btn btn-secondary" disabled>
              {tx("Coming soon")}
            </button>
          )}
        </Row>
        {CLOUD_LIVE && !signedIn && <p className="fine">{tx("Sign in to your Mailzo account first, in Profile.")}</p>}
        <Row
          icon={CloudServerIcon}
          label={tx("Your own server")}
          help={tx("Free. Run Mailzo Server on a computer of your own that's always on.")}
        >
          <button className="btn btn-secondary" disabled={busy} onClick={() => setOwn(true)}>
            {tx("Connect my own server")}
          </button>
        </Row>
      </Section>
      {error && <Problem text={error} />}
    </Page>
  );
}

/** Address and code, then the same six numbers on both screens. */
function PairOwnServer({ onDone }: { onDone: () => void }) {
  const [url, setUrl] = useState("");
  const [code, setCode] = useState("");
  const [numbers, setNumbers] = useState<string | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.helperStatus().then((s) => s.pending && setNumbers(s.pending.numbers), () => {});
  }, []);

  // Once the person said the numbers match here, wait for them to say so on the server.
  useEffect(() => {
    if (!waiting) return;
    const timer = setInterval(() => {
      api.helperPairWait().then(
        (r) => {
          if (r.state === "approved") onDone();
          if (r.state === "refused") {
            setWaiting(false);
            setNumbers(null);
            setError(tx("The server said the numbers didn't match, so nothing was connected."));
          }
        },
        (e) => {
          setWaiting(false);
          setNumbers(null);
          setError(say(e));
        },
      );
    }, 2000);
    return () => clearInterval(timer);
  }, [waiting, onDone]);

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      setNumbers((await api.helperPairStart(url, code)).numbers);
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    void api.helperPairCancel();
    onDone();
  };

  if (numbers) {
    return (
      <Page title={tx("Connect my own server")}>
        <p className="body-text">
          {waiting
            ? url.trim()
              ? fill(tx("To finish, click They match on {0} too."), <b>{url.trim().replace(/^https?:\/\//, "").replace(/\/.*$/, "")}</b>)
              : tx("Now click They match on your server's page too.")
            : tx("Check your server's page shows these same numbers, then click They match.")}
        </p>
        <div aria-label={tx("Numbers to match")} className="selectable number-box">
          {numbers}
        </div>
        {error && <Problem text={error} />}
        {waiting ? (
          <div className="hstack">
            <Spinner />
            <span className="fine">{tx("Waiting for your server…")}</span>
          </div>
        ) : (
          <div className="hstack">
            <button className="btn btn-secondary" onClick={cancel}>
              {tx("They don't match")}
            </button>
            <button
              className="btn btn-primary"
              onClick={() =>
                api.helperPairConfirm().then(
                  () => setWaiting(true),
                  (e) => setError(say(e)),
                )
              }
            >
              {tx("They match")}
            </button>
          </div>
        )}
      </Page>
    );
  }

  return (
    <Page
      title={tx("Connect my own server")}
      intro={tx("On your server, open Settings › Connect and make a pairing code.")}
    >
      <form
        className="stack"
        onSubmit={(e) => {
          e.preventDefault();
          void start();
        }}
      >
        <label className="field">
          <span className="field-label">{tx("Server address")}</span>
          <input
            className="input"
            placeholder="mail.example.com"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
        </label>
        <label className="field">
          <span className="field-label">{tx("Pairing code")}</span>
          <input
            className="input"
            placeholder="ABCD-EFGH"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            autoCapitalize="characters"
            autoCorrect="off"
            spellCheck={false}
          />
        </label>
        {error && <Problem text={error} />}
        <div className="hstack">
          <button type="button" className="btn btn-secondary" onClick={cancel}>
            {tx("Cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || !url.trim() || !code.trim()}>
            {busy ? <Spinner /> : tx("Connect")}
          </button>
        </div>
      </form>
    </Page>
  );
}

/** On the web: make a code for a Mac, compare the numbers, and see paired devices. */
function ConnectAMac() {
  const [pair, setPair] = useState<PairStatus | null>(null);
  const [devices, setDevices] = useState<PairedDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState<PairedDevice | null>(null);
  const [copied, setCopied] = useState(false);
  const [copiedAddress, setCopiedAddress] = useState(false);

  const load = useCallback(() => {
    api.pairStatus().then(setPair, (e) => setError(say(e)));
    api.pairDevices().then((r) => setDevices(r.devices), (e) => setError(say(e)));
  }, []);
  useEffect(load, [load]);

  // While a code is out, keep looking for the device.
  useEffect(() => {
    if (pair?.stage !== "waiting" && pair?.stage !== "connecting" && pair?.stage !== "confirm") return;
    const timer = setInterval(() => {
      api.pairStatus().then((p) => {
        setPair(p);
        if (p.stage === "done" || p.stage === "none") load();
        // The Mac sends its inboxes straight after, so the mail screens are opened again
        // with them.
        if (p.stage === "done") setTimeout(() => window.location.reload(), 5000);
      }, () => {});
    }, 2000);
    return () => clearInterval(timer);
  }, [pair?.stage, load]);

  const run = (work: Promise<unknown>) => work.then(load, (e) => setError(say(e)));
  const copy = (text: string, done: (on: boolean) => void = setCopied) =>
    copyText(text).then(
      () => {
        done(true);
        setTimeout(() => done(false), 1500);
      },
      (e) => setError(say(e)),
    );

  return (
    <Page title={tx("Connect")} intro={tx("Keep filters, scheduled emails and reminders working while your Mac is off.")}>
      {pair?.stage === "confirm" ? (
        <div className="pair-step">
          <p className="body-text">
            {tx("Do these match the numbers on {0}?").replace("{0}", pair.device_name || tx("your Mac"))}
          </p>
          <div aria-label={tx("Numbers to match")} className="selectable number-box">
            {pair.numbers}
          </div>
          <div className="hstack">
            <button className="btn btn-secondary" onClick={() => run(api.pairApprove(false))}>
              {tx("They don't match")}
            </button>
            <button className="btn btn-primary" onClick={() => run(api.pairApprove(true))}>
              {tx("They match")}
            </button>
          </div>
        </div>
      ) : pair?.stage === "connecting" ? (
        <div className="pair-step">
          <p className="body-text">{tx("Your Mac is connecting. Check it shows these same numbers, then click They match on your Mac.")}</p>
          <div aria-label={tx("Numbers to match")} className="selectable number-box">
            {pair.numbers}
          </div>
          <div className="hstack">
            <Spinner />
            <span className="fine">{tx("Waiting for your Mac…")}</span>
          </div>
        </div>
      ) : pair?.stage === "waiting" && pair.code ? (
        <div className="pair-step">
          <button
            type="button"
            className="number-box pair-code"
            aria-label={`${tx("Copy pairing code")} ${pair.code}`}
            onClick={() => void copy(pair.code ?? "")}
          >
            <span>{pair.code}</span>
            <span className={`pair-code-hint ${copied ? "pair-code-hint-on" : ""}`}>{copied ? tx("Copied") : tx("Click to copy")}</span>
          </button>
          <ol className="pair-steps">
            <li>
              <span className="pair-num">1</span>
              <span>{fill(tx("On your Mac, open {0}."), <b>{tx("Settings › Connect › Connect my own server")}</b>)}</span>
            </li>
            <li>
              <span className="pair-num">2</span>
              <span>
                {fill(
                  tx("Enter {0} and this code."),
                  <button
                    type="button"
                    className="pair-chip"
                    data-tip={copiedAddress ? tx("Copied") : tx("Click to copy")}
                    aria-label={`${tx("Copy address")} ${window.location.host}`}
                    onClick={() => void copy(window.location.host, setCopiedAddress)}
                  >
                    {window.location.host}
                    <Icon icon={copiedAddress ? Tick02Icon : Copy01Icon} size="xs" />
                  </button>,
                )}
              </span>
            </li>
            <li>
              <span className="pair-num">3</span>
              <span>{tx("Your Mac then shows six numbers. They show here too, to check they match.")}</span>
            </li>
          </ol>
          <p className="fine">{tx("The code works once, for ten minutes.")}</p>
        </div>
      ) : (
        <div className="pair-step">
          <button className="btn btn-primary" onClick={() => run(api.pairNew())}>
            {tx("Make a pairing code")}
          </button>
        </div>
      )}
      {pair?.stage === "done" && <p className="body-text">{tx("Connected. Your inboxes and settings are on their way from your Mac.")}</p>}
      {pair?.stage === "refused" && <Problem text={tx("Nothing was connected.")} />}
      {error && <Problem text={error} />}
      <Section title={tx("Connected devices")}>
        {devices === null ? (
          <Spinner />
        ) : devices.length === 0 ? (
          <Row icon={LaptopIcon} label={tx("No devices yet")} />
        ) : (
          devices.map((d) => (
            <Row
              key={d.device_id}
              icon={LaptopIcon}
              label={d.name}
              help={d.last_seen ? `${tx("Last heard from")} ${when(d.last_seen)}` : tx("Not heard from yet")}
            >
              <button className="btn btn-secondary" aria-label={tx("Remove")} onClick={() => setRemoving(d)}>
                {tx("Remove")}
              </button>
            </Row>
          ))
        )}
      </Section>
      {removing && (
        <Confirm
          title={tx("Remove {0}?").replace("{0}", removing.name)}
          confirm={tx("Remove")}
          keep={tx("Keep it")}
          danger
          onKeep={() => setRemoving(null)}
          onConfirm={() => {
            const d = removing;
            setRemoving(null);
            void run(api.pairRemove(d.device_id));
          }}
        >
          {tx("It stops keeping in step with this server. Scheduled emails it already handed over still go.")}
        </Confirm>
      )}
    </Page>
  );
}

