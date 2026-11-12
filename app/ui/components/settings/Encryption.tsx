import { FormEvent, useCallback, useEffect, useState } from "react";
import { ApiError, pgpApi, type PgpKey, type PgpStatus } from "../../api";
import { Copy01Icon, SquareLock02Icon } from "../../icons";
import type { Account } from "../../types";
import { Confirm, Dialog, Icon, Problem, Spinner } from "../ui";
import { Row, Section } from "./controls";
import { tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/** Groups of four, as people read fingerprints out to each other. */
const spaced = (fingerprint: string) => fingerprint.replace(/(.{4})/g, "$1 ").trim();

async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** One inbox's own key: made here, or brought from another app. */
export function EncryptionSection({ account }: { account: Account }) {
  const [status, setStatus] = useState<PgpStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [copied, setCopied] = useState(false);

  const load = useCallback(() => {
    pgpApi
      .status(account.id)
      .then(setStatus)
      .catch((e) => setProblem(say(e)));
  }, [account.id]);
  useEffect(load, [load]);

  const run = async (work: () => Promise<PgpStatus | unknown>) => {
    setBusy(true);
    setProblem(null);
    try {
      await work();
      load();
    } catch (e) {
      setProblem(say(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section
      title={tx("Encrypted email")}
      note={tx("With a key, people can send you email only you can read. It stays in this Mac's Keychain, and only the public half is shared.")}
    >
      {!status && !problem && (
        <div className="group-row muted">
          <Spinner /> {" "}{tx("Loading…")}</div>
      )}
      {status && !status.has_key && (
        <Row icon={SquareLock02Icon} label={tx("No key yet")} help={tx("Make one, or bring one from another app.")}>
          <span className="hstack">
            <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => setImporting(true)}>
              {tx("Import a key")}</button>
            <button className="btn btn-primary btn-sm" disabled={busy} onClick={() => void run(() => pgpApi.generate(account.id))}>
              {busy && <Spinner size="xs" />}
              {tx("Make a key")}</button>
          </span>
        </Row>
      )}
      {status?.has_key && (
        <>
          <Row
            icon={SquareLock02Icon}
            label={tx("Your key")}
            help={<span className="selectable tnum">{status.fingerprint ? spaced(status.fingerprint) : ""}</span>}
          >
            <span className="hstack">
              {status.public_key && (
                <button
                  className="btn btn-secondary btn-sm"
                  onClick={() =>
                    void copy(status.public_key ?? "").then((ok) => {
                      setCopied(ok);
                      if (ok) setTimeout(() => setCopied(false), 2000);
                    })
                  }
                >
                  <Icon icon={Copy01Icon} size="xs" />
                  {copied ? tx("Copied") : tx("Copy public key")}
                </button>
              )}
              <button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setConfirmRemove(true)}>
                {tx("Remove")}</button>
            </span>
          </Row>
        </>
      )}
      <Problem text={problem} />
      {importing && (
        <ImportSecret
          onClose={() => setImporting(false)}
          onImport={async (armored, passphrase) => {
            await pgpApi.importSecret(account.id, armored, passphrase || undefined);
            setImporting(false);
            load();
          }}
        />
      )}
      {confirmRemove && (
        <Confirm
          title={tx("Remove your key?")}
          confirm={tx("Yes, remove")}
          keep={tx("Keep it")}
          danger
          onKeep={() => setConfirmRemove(false)}
          onConfirm={() => {
            setConfirmRemove(false);
            void run(() => pgpApi.remove(account.id));
          }}
        >
          {tx("Email already encrypted to this key can't be read here without it. If you use it in another app, it stays there.")}</Confirm>
      )}
    </Section>
  );
}

function ImportSecret({
  onClose,
  onImport,
}: {
  onClose: () => void;
  onImport: (armored: string, passphrase: string) => Promise<void>;
}) {
  const [armored, setArmored] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setProblem(null);
    try {
      await onImport(armored, passphrase);
    } catch (err) {
      setProblem(say(err));
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={tx("Import your key")}
      subtitle={tx("Paste the private key from your other app, like Thunderbird or Proton.")}
      onClose={onClose}
      onSubmit={submit}
      footer={
        <>
          <button type="button" className="btn btn-secondary" onClick={onClose}>
            {tx("Cancel")}</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !armored.trim()}>
            {busy && <Spinner size="xs" />}
            {tx("Import")}</button>
        </>
      }
    >
      <div className="stack">
        <label className="field">
          <span className="field-label">{tx("Private key")}</span>
          <textarea
            required
            rows={8}
            className="mono-input"
            placeholder="-----BEGIN PGP PRIVATE KEY BLOCK-----"
            value={armored}
            onChange={(e) => setArmored(e.target.value)}
            spellCheck={false}
          />
        </label>
        <label className="field">
          <span className="field-label">{tx("Its passphrase, if it has one")}</span>
          <input type="password" value={passphrase} onChange={(e) => setPassphrase(e.target.value)} autoComplete="off" />
        </label>
        <Problem text={problem} />
      </div>
    </Dialog>
  );
}

/** Other people's public keys: learned from their email, or pasted in. */
export function OthersKeys() {
  const [keys, setKeys] = useState<PgpKey[] | null>(null);
  const [adding, setAdding] = useState(false);
  const [text, setText] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const load = useCallback(() => {
    pgpApi
      .keys()
      .then(setKeys)
      .catch(() => setKeys([]));
  }, []);
  useEffect(load, [load]);

  const add = async (e: FormEvent) => {
    e.preventDefault();
    setProblem(null);
    try {
      await pgpApi.importKey(text);
      setAdding(false);
      setText("");
      load();
    } catch (err) {
      setProblem(say(err));
    }
  };

  return (
    <Section
      title={tx("Other people's encryption keys")}
      note={tx("Keys arrive when someone using encryption emails you. Or paste one in.")}
    >
      {keys?.length === 0 && <Row label={tx("None yet")} />}
      {keys?.map((k) => (
        <Row
          key={k.address}
          label={k.address}
          help={
            <>
              <span className="tnum">
                {spaced(k.fingerprint)} · {k.source === "imported" ? tx("added by you") : tx("from their email")}
              </span>
              {k.changed && k.new_fingerprint && (
                <span className="health-bad">
                  {" "}
                  {tx("A newer email from them offered a different key: {fingerprint}. The one above is still used. Only switch if they told you they changed keys.", {
                    fingerprint: spaced(k.new_fingerprint),
                  })}
                </span>
              )}
            </>
          }
        >
          {k.changed && (
            <button
              className="btn btn-secondary btn-sm"
              onClick={() => void pgpApi.acceptKeyChange(k.address).then(load).catch((e) => setProblem(say(e)))}
            >
              {tx("Use the new key")}</button>
          )}
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => void pgpApi.deleteKey(k.address).then(load).catch((e) => setProblem(say(e)))}
          >
            {tx("Remove")}</button>
        </Row>
      ))}
      <Row label={tx("Add someone's key")}>
        <button className="btn btn-secondary btn-sm" onClick={() => setAdding(true)}>
          {tx("Paste a key")}</button>
      </Row>
      <Problem text={problem} />
      {adding && (
        <Dialog
          title={tx("Add someone's key")}
          subtitle={tx("Paste their public key.")}
          onClose={() => setAdding(false)}
          onSubmit={add}
          footer={
            <>
              <button type="button" className="btn btn-secondary" onClick={() => setAdding(false)}>
                {tx("Cancel")}</button>
              <button type="submit" className="btn btn-primary" disabled={!text.trim()}>
                {tx("Add")}</button>
            </>
          }
        >
          <textarea
            required
            rows={8}
            className="mono-input"
            aria-label={tx("Public key")}
            placeholder="-----BEGIN PGP PUBLIC KEY BLOCK-----"
            value={text}
            onChange={(e) => setText(e.target.value)}
            spellCheck={false}
          />
          <Problem text={problem} />
        </Dialog>
      )}
    </Section>
  );
}
