import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { ApiError, api, type LinkedDevice } from "../../api";
import { Delete02Icon, Key01Icon, UserRemove01Icon, LaptopIcon, Logout03Icon, SecurityCheckIcon, UserGroupIcon } from "../../icons";
import { RecoveryKey } from "../RecoveryKey";
import { Avatar, Confirm, Dialog, Problem, Spinner } from "../ui";
import { Page, Row, Section } from "./controls";
import { tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));

/** With no Mailzo account: what one is for, and a way to start it. Nothing here is needed to read mail. */
export function NoAccountYet({ onSetUpAccount }: { onSetUpAccount: () => void }) {
  return (
    <Page
      title={tx("Your account")}
      intro={tx("Not needed on this device. Add a free one for sync and teams.")}
    >
      <Section>
        <Row icon={LaptopIcon} label={tx("Your inboxes on every device")} help={tx("Your inboxes and settings on every device.")} />
        <Row icon={UserGroupIcon} label={tx("Work with a team")} help={tx("Share inboxes, assign emails and leave private notes.")} />
        <Row
          icon={SecurityCheckIcon}
          label={tx("Locked before it leaves this device")}
          help={tx("Only your devices can read what's synced. Not even us.")}
        />
      </Section>
      <button className="btn btn-primary btn-block" onClick={onSetUpAccount}>
        {tx("Set up a free account")}</button>
    </Page>
  );
}

export function AccountPage({
  email,
  onSignedOut,
  onSetUpAccount,
}: {
  email: string;
  onSignedOut: () => void;
  onSetUpAccount: () => void;
}) {
  if (!email) return <NoAccountYet onSetUpAccount={onSetUpAccount} />;
  return <SignedInAccount email={email} onSignedOut={onSignedOut} />;
}

function SignedInAccount({ email, onSignedOut }: { email: string; onSignedOut: () => void }) {
  const [devices, setDevices] = useState<LinkedDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<LinkedDevice | null>(null);
  const [confirmSignOut, setConfirmSignOut] = useState(false);
  const [confirmErase, setConfirmErase] = useState(false);
  const [confirmNewKey, setConfirmNewKey] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  /** The emailed code was sent, so the person can type it in to delete the account. */
  const [deleting, setDeleting] = useState(false);
  const [newKey, setNewKey] = useState<string | null>(null);
  const [makingKey, setMakingKey] = useState(false);

  const load = useCallback(() => {
    api
      .listDevices()
      .then(setDevices)
      .catch((e) => setError(say(e)));
  }, []);

  useEffect(load, [load]);

  const remove = async (id: number) => {
    setError(null);
    setConfirmRemove(null);
    try {
      await api.removeDevice(id);
      load();
    } catch (e) {
      setError(say(e));
    }
  };

  const leave = async (how: () => Promise<unknown>) => {
    setError(null);
    setConfirmSignOut(false);
    setConfirmErase(false);
    try {
      await how();
      onSignedOut();
    } catch (e) {
      setError(say(e));
    }
  };

  const startDelete = async () => {
    setError(null);
    setConfirmDelete(false);
    try {
      await api.accountDeleteStart();
      setDeleting(true);
    } catch (e) {
      setError(say(e));
    }
  };

  const makeKey = async () => {
    // Each new key replaces the last, so a second one made meanwhile would undo the first.
    if (makingKey) return;
    setMakingKey(true);
    setError(null);
    setConfirmNewKey(false);
    try {
      setNewKey((await api.newRecoveryKey()).recovery_key);
    } catch (e) {
      setError(say(e));
    } finally {
      setMakingKey(false);
    }
  };

  if (newKey) {
    return createPortal(
      <div className="scrim scrim-full">
        <RecoveryKey value={newKey} onDone={() => setNewKey(null)} />
      </div>,
      document.body,
    );
  }

  return (
    <Page title={tx("Your account")}>
      <Section title={tx("Main account email")}>
        <div className="group-row">
          <Avatar name={email} />
          <span className="grow truncate-1 strong">{email}</span>
        </div>
      </Section>

      <Section
        title={tx("Your devices")}
        note={tx("A removed device erases its mail when it next goes online. If it was lost, also change your email passwords.")}
      >
        {devices === null && !error && (
          <div className="group-row muted">
            <Spinner /> {" "}{tx("Loading…")}</div>
        )}
        {devices?.map((d) => (
          <Row
            key={d.id}
            icon={LaptopIcon}
            label={
              <>
                {d.name}
                {d.is_self && <span className="pill">{tx("this device")}</span>}
                {d.signed_out && <span className="pill">{tx("signed out")}</span>}
              </>
            }
          >
            {!d.is_self && (
              <button className="btn btn-ghost btn-sm" onClick={() => setConfirmRemove(d)}>
                {tx("Remove")}</button>
            )}
          </Row>
        ))}
      </Section>

      <Section title={tx("Security")}>
        <Row icon={Key01Icon} label={tx("Recovery key")} help={tx("Gets your inboxes back if you lose every device.")}>
          <button className="btn btn-secondary btn-sm" disabled={makingKey} onClick={() => setConfirmNewKey(true)}>
            {makingKey && <Spinner size="xs" />}
            {tx("Make a new recovery key")}</button>
        </Row>
        <Row icon={Logout03Icon} label={tx("Sign out")} help={tx("Sign back in any time with an emailed code.")}>
          <button className="btn btn-secondary btn-sm" onClick={() => setConfirmSignOut(true)}>
            {tx("Sign out of this device")}</button>
        </Row>
        <Row
          icon={Delete02Icon}
          label={tx("Erase this device")}
          help={tx("Removes this computer from your account, for selling or lending it.")}
        >
          <button className="btn btn-secondary btn-sm" onClick={() => setConfirmErase(true)}>
            {tx("Erase this device")}</button>
        </Row>
      </Section>

      <Section>
        <Row
          icon={UserRemove01Icon}
          label={tx("Delete your Mailzo account")}
          help={tx("This device keeps its inboxes and mail.")}
        >
          <button className="btn btn-secondary btn-sm" onClick={() => setConfirmDelete(true)}>
            {tx("Delete account")}</button>
        </Row>
      </Section>

      <Problem text={error} />

      {confirmRemove && (
        <Confirm
          title={`Remove ${confirmRemove.name}?`}
          confirm={tx("Yes, remove")}
          keep={tx("Keep")}
          danger
          onKeep={() => setConfirmRemove(null)}
          onConfirm={() => void remove(confirmRemove.id)}
        >
          {tx("It loses your inboxes, and its mail is erased when it next goes online.")}</Confirm>
      )}
      {confirmNewKey && (
        <Confirm
          title={tx("Make a new recovery key?")}
          confirm={tx("Yes, make a new key")}
          keep={tx("Keep the old one")}
          onKeep={() => setConfirmNewKey(false)}
          onConfirm={() => void makeKey()}
        >
          {tx("Your old recovery key will stop working.")}</Confirm>
      )}
      {confirmSignOut && (
        <Confirm
          title={tx("Sign out of this device?")}
          confirm={tx("Yes, sign out")}
          keep={tx("Stay signed in")}
          onKeep={() => setConfirmSignOut(false)}
          onConfirm={() => void leave(api.signOut)}
        >
          {tx("Sign back in with an emailed code to get your inboxes and settings back. Drafts only on this device are erased.")}</Confirm>
      )}
      {confirmDelete && (
        <Confirm
          title={tx("Delete your Mailzo account?")}
          confirm={tx("Yes, email me a code")}
          keep={tx("Keep my account")}
          danger
          onKeep={() => setConfirmDelete(false)}
          onConfirm={() => void startDelete()}
        >
          {tx("Deletes your account for good: devices, synced settings, recovery key, teams and plan. Your email stays with your providers, and this device keeps its mail. We'll email you a code first.")}</Confirm>
      )}
      {deleting && (
        <DeleteAccountCode
          email={email}
          onClose={() => setDeleting(false)}
          onDeleted={() => {
            setDeleting(false);
            onSignedOut();
          }}
        />
      )}
      {confirmErase && (
        <Confirm
          title={tx("Erase this device?")}
          confirm={tx("Yes, erase")}
          keep={tx("Keep it")}
          danger
          onKeep={() => setConfirmErase(false)}
          onConfirm={() => void leave(api.eraseDevice)}
        >
          {tx("Removes all mail, passwords and drafts here and takes this device off your account. To come back, you'll need another device or your recovery key. Your mail stays with your provider.")}</Confirm>
      )}
    </Page>
  );
}

/** The last step of deleting the account: the code emailed just now. */
function DeleteAccountCode({
  email,
  onClose,
  onDeleted,
}: {
  email: string;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [resent, setResent] = useState(false);
  const [resending, setResending] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setProblem(null);
    try {
      await api.accountDelete(code.trim());
      onDeleted();
    } catch (err) {
      setProblem(say(err));
      setBusy(false);
    }
  };

  const resend = async () => {
    // One new code at a time, or the first one emailed stops working.
    if (resending) return;
    setResending(true);
    setProblem(null);
    try {
      await api.accountDeleteStart();
      setResent(true);
    } catch (err) {
      setProblem(say(err));
    } finally {
      setResending(false);
    }
  };

  return (
    <Dialog
      title={tx("Check your email")}
      subtitle={
        <>
          {tx("We sent a 6-digit code to")}{" "}{email}.
        </>
      }
      width="s"
      onClose={onClose}
      onSubmit={submit}
      footer={
        <>
          <button type="button" className="btn btn-secondary" onClick={onClose}>
            {tx("Cancel")}</button>
          <button type="submit" className="btn btn-danger" disabled={busy || !code.trim()}>
            {busy && <Spinner size="xs" />}
            {tx("Delete account")}</button>
        </>
      }
    >
      <div className="stack">
        <input
          required
          autoFocus
          aria-label={tx("Code")}
          className="code-input"
          placeholder="000000"
          inputMode="numeric"
          autoComplete="one-time-code"
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        <Problem text={problem} />
        {resent && <p className="fine">{tx("We sent a new code.")}</p>}
        <button type="button" className="btn btn-ghost btn-sm" disabled={resending} onClick={() => void resend()}>
          {resending && <Spinner size="xs" />}
          {tx("Send again")}</button>
      </div>
    </Dialog>
  );
}
