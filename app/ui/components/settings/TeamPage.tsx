import { FormEvent, useCallback, useEffect, useState } from "react";
import { ApiError, api, type TeamInvite, type TeamStatus, type TeamTemplate } from "../../api";
import { Add01Icon, ArrowLeft01Icon, ArrowRight01Icon, Copy01Icon, Delete02Icon, Edit02Icon, InboxIcon, Note01Icon, Tick02Icon, UserGroupIcon } from "../../icons";
import type { Account } from "../../types";
import { Avatar, Confirm, Dialog, Icon, IconButton, Problem, Spinner } from "../ui";
import { Page, Row, Section, Segmented } from "./controls";
import { language, tx } from "../../i18n";

const say = (e: unknown) => (e instanceof ApiError ? e.message : String(e));
const ROLE_NAMES: Record<string, string> = { owner: "Owner", admin: "Admin", member: "Member" };

export function TeamPage({
  accounts,
  signedIn,
  onSetUpAccount,
  onChanged,
}: {
  accounts: Account[];
  /** Teams need a Mailzo account, which is optional for using Mailzo alone. */
  signedIn: boolean;
  onSetUpAccount: () => void;
  onChanged: () => void;
}) {
  if (!signedIn) {
    return (
      <Page title={tx("Teams")} intro={tx("Share inboxes, assign emails and leave private notes for each other.")}>
        <Section note={tx("Teams need a free Mailzo account.")}>
          <Row icon={UserGroupIcon} label={tx("Start or join a team")}>
            <button className="btn btn-primary btn-sm" onClick={onSetUpAccount}>
              {tx("Set up a free account")}</button>
          </Row>
        </Section>
      </Page>
    );
  }
  return <Teams accounts={accounts} onChanged={onChanged} />;
}

function Teams({ accounts, onChanged }: { accounts: Account[]; onChanged: () => void }) {
  const [teams, setTeams] = useState<TeamStatus[] | null>(null);
  const [openId, setOpenId] = useState<number | null>(null);

  const load = useCallback(async () => {
    setTeams((await api.teamsStatus()).teams);
    try {
      setTeams((await api.teamsRefresh()).teams);
    } catch {
      // What's saved on this device is still worth showing.
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const changed = () => {
    onChanged();
    void load();
  };

  if (!teams) {
    return (
      <Page title={tx("Teams")}>
        <Spinner />
      </Page>
    );
  }
  const open = teams.find((t) => t.team_id === openId);
  if (open) {
    return <OneTeam team={open} accounts={accounts} onBack={() => setOpenId(null)} onChanged={changed} />;
  }
  const added = (team: TeamStatus) => {
    setOpenId(team.team_id ?? null);
    changed();
  };
  if (teams.length === 0) {
    return (
      <Page title={tx("Teams")} intro={tx("Handle shared email together.")}>
        <Section note={tx("Mail stays on each person's devices. We can't read notes or passwords.")}>
          <Row icon={InboxIcon} label={tx("Share inboxes")} help={tx("Everyone works from the same support@ or sales@.")} />
          <Row icon={UserGroupIcon} label={tx("Assign emails")} help={tx("See who's handling what, so nothing gets answered twice.")} />
          <Row icon={Note01Icon} label={tx("Leave private notes")} help={tx("Talk about an email without the sender seeing.")} />
        </Section>
        <NewTeam onDone={added} />
      </Page>
    );
  }
  return (
    <Page
      title={tx("Teams")}
      intro={tx("Each team has its own people, inboxes, notes and templates. We can't read notes or passwords.")}
    >
      <Section title={tx("Your teams")}>
        {teams.map((t) => (
          <TeamRow key={t.team_id} team={t} accounts={accounts} onOpen={() => setOpenId(t.team_id ?? null)} />
        ))}
      </Section>
      <NewTeam onDone={added} />
    </Page>
  );
}

const people = (count: number) => (count === 1 ? tx("1 person") : `${count} people`);
const sharedInboxes = (count: number) => (count === 1 ? tx("1 shared inbox") : tx("{count} shared inboxes", { count }));

function TeamRow({ team, accounts, onOpen }: { team: TeamStatus; accounts: Account[]; onOpen: () => void }) {
  const inboxes = accounts.filter((a) => a.prefs.team_shared && a.prefs.team_id === team.team_id).length;
  const about =
    team.state === "ready"
      ? [ROLE_NAMES[team.role ?? ""] ?? team.role, people(team.members?.length ?? 0), sharedInboxes(inboxes)].join(" · ")
      : tx("Waiting for this team's key to reach this device");
  return (
    <button type="button" className="group-row team-row" data-team={team.name} onClick={onOpen}>
      <Avatar name={team.name || "Team"} />
      <div className="grow">
        <div className="row-label truncate-1">{team.name || tx("A team")}</div>
        <div className="row-help">{about}</div>
      </div>
      <Icon icon={ArrowRight01Icon} size="s" className="muted" />
    </button>
  );
}

function OneTeam({
  team,
  accounts,
  onBack,
  onChanged,
}: {
  team: TeamStatus;
  accounts: Account[];
  onBack: () => void;
  onChanged: () => void;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  const teamId = team.team_id ?? 0;
  const canManage = team.role !== "member";
  const isOwner = team.role === "owner";
  const gone = () => {
    onBack();
    onChanged();
  };
  const back = (
    <button type="button" className="btn btn-ghost btn-sm team-back" onClick={onBack}>
      <Icon icon={ArrowLeft01Icon} size="s" />
      {tx("All teams")}</button>
  );
  if (team.state !== "ready") {
    return (
      <Page title={team.name || tx("Team")} action={back}>
        <Problem text={tx("This device doesn't have the team's key yet. Open Mailzo on a device already in the team, then try again.")} />
        <Problem text={problem} />
        <LeaveTeam teamId={teamId} isOwner={isOwner} onDone={gone} onProblem={setProblem} />
      </Page>
    );
  }
  return (
    <Page
      title={team.name || tx("Team")}
      action={back}
      intro={tx("Share inboxes, assign emails and leave private notes. We can't read notes or passwords.")}
    >
      <Members status={team} teamId={teamId} canManage={canManage} isOwner={isOwner} onChanged={onChanged} />
      {canManage && <Invites teamId={teamId} isOwner={isOwner} />}
      <SharedInboxes teamId={teamId} accounts={accounts} canManage={canManage} onChanged={onChanged} />
      <Templates teamId={teamId} />
      <Problem text={problem} />
      <LeaveTeam teamId={teamId} isOwner={isOwner} onDone={gone} onProblem={setProblem} />
    </Page>
  );
}

function NewTeam({ onDone }: { onDone: (team: TeamStatus) => void }) {
  const [asking, setAsking] = useState<"start" | "join" | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const open = (what: "start" | "join") => {
    setText("");
    setProblem(null);
    setAsking(what);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setProblem(null);
    try {
      const team = await (asking === "start" ? api.teamCreate(text) : api.teamJoin(text));
      setAsking(null);
      onDone(team);
    } catch (err) {
      setProblem(say(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="hstack-m">
        <button type="button" className="btn btn-primary" onClick={() => open("start")}>
          {tx("Start a team")}</button>
        <button type="button" className="btn btn-secondary" onClick={() => open("join")}>
          {tx("I have an invite code")}</button>
      </div>
      {asking && (
        <Dialog
          title={asking === "start" ? tx("Start a team") : tx("Join a team")}
          width="s"
          onClose={() => setAsking(null)}
          onSubmit={(e) => void submit(e)}
          footer={
            <>
              <button type="button" className="btn btn-ghost" onClick={() => setAsking(null)}>
                {tx("Cancel")}</button>
              <button type="submit" className="btn btn-primary" disabled={busy || !text.trim()}>
                {busy && <Spinner size="xs" />}
                {asking === "start" ? tx("Create team") : tx("Join team")}
              </button>
            </>
          }
        >
          <div className="stack">
            {asking === "start" ? (
              <label className="field">
                <span className="field-label">{tx("Team name")}</span>
                <input
                  autoFocus
                  aria-label={tx("Team name")}
                  placeholder={tx("Acme")}
                  value={text}
                  maxLength={80}
                  onChange={(e) => setText(e.target.value)}
                />
              </label>
            ) : (
              <label className="field">
                <span className="field-label">{tx("Invite code")}</span>
                <input
                  autoFocus
                  aria-label={tx("Invite code")}
                  value={text}
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  onChange={(e) => setText(e.target.value)}
                />
              </label>
            )}
            <div className="fine">
              {asking === "start"
                ? tx("You can invite people right after this.")
                : tx("Ask someone in the team to send you a code.")}
            </div>
            <Problem text={problem} />
          </div>
        </Dialog>
      )}
    </>
  );
}

function Members({
  status,
  teamId,
  canManage,
  isOwner,
  onChanged,
}: {
  status: TeamStatus;
  teamId: number;
  canManage: boolean;
  isOwner: boolean;
  onChanged: () => void;
}) {
  const [removing, setRemoving] = useState<{ user_id: number; email: string } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const remove = async (id: number) => {
    setRemoving(null);
    setProblem(null);
    try {
      await api.teamRemoveMember(teamId, id);
      onChanged();
    } catch (e) {
      setProblem(say(e));
    }
  };

  const setRole = async (id: number, role: string) => {
    setProblem(null);
    try {
      await api.teamSetRole(teamId, id, role);
      onChanged();
    } catch (e) {
      setProblem(say(e));
    }
  };

  return (
    <Section title={tx("People")}>
      {status.members?.map((m) => {
        const me = m.user_id === status.me;
        const removable =
          !me && m.role !== "owner" && (isOwner || (canManage && m.role === "member"));
        return (
          <div key={m.user_id} className="group-row" data-member={m.email}>
            <Avatar name={m.email} />
            <span className="grow truncate-1 strong">
              {m.email}
              {me && <span className="pill">you</span>}
            </span>
            {isOwner && !me ? (
              <Segmented
                label={tx("Role for {0}", { "0": m.email })}
                value={m.role}
                options={[
                  { value: "member", label: tx("Member") },
                  { value: "admin", label: tx("Admin") },
                ]}
                onChange={(role) => void setRole(m.user_id, role)}
              />
            ) : (
              <span className="muted">{ROLE_NAMES[m.role] ?? m.role}</span>
            )}
            {removable && (
              <button className="btn btn-ghost btn-sm" onClick={() => setRemoving(m)}>
                {tx("Remove")}</button>
            )}
          </div>
        );
      })}
      <Problem text={problem} />
      {removing && (
        <Confirm
          title={`Remove ${removing.email}?`}
          confirm={tx("Yes, remove")}
          keep={tx("Keep")}
          danger
          onKeep={() => setRemoving(null)}
          onConfirm={() => void remove(removing.user_id)}
        >
          {tx("Shared inboxes leave their devices the next time they're online, and their emails go back to being unassigned. Their devices have held the inbox passwords, so change them if this person left on bad terms.")}</Confirm>
      )}
    </Section>
  );
}

function Invites({ teamId, isOwner }: { teamId: number; isOwner: boolean }) {
  const [role, setRole] = useState("member");
  const [code, setCode] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [waiting, setWaiting] = useState<TeamInvite[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const [making, setMaking] = useState(false);

  const load = useCallback(() => {
    api
      .teamInvites(teamId)
      .then(setWaiting)
      .catch(() => {});
  }, [teamId]);
  useEffect(load, [load]);

  const invite = async () => {
    // Each click makes a code that works, so a double click would leave a spare one about.
    if (making) return;
    setMaking(true);
    setProblem(null);
    setCopied(false);
    try {
      setCode((await api.teamInvite(teamId, role)).code);
      load();
    } catch (e) {
      setProblem(say(e));
    } finally {
      setMaking(false);
    }
  };

  return (
    <Section
      title={tx("Invite someone")}
      note={tx("Send them the code yourself. It works once, for 7 days. We never see it.")}
    >
      <div className="group-row">
        {isOwner && (
          <Segmented
            label={tx("Invite as")}
            value={role}
            options={[
              { value: "member", label: tx("Member") },
              { value: "admin", label: tx("Admin") },
            ]}
            onChange={setRole}
          />
        )}
        <span className="grow" />
        <button className="btn btn-primary btn-sm" disabled={making} onClick={() => void invite()}>
          {making ? <Spinner size="xs" /> : <Icon icon={Add01Icon} size="s" />}
          {tx("Make an invite code")}</button>
      </div>
      {code && (
        <div className="group-row team-code">
          <span aria-label={tx("Invite code")} className="selectable key-box grow">
            {code}
          </span>
          <button
            className="btn btn-secondary btn-sm"
            onClick={() => {
              void navigator.clipboard.writeText(code).then(() => setCopied(true));
            }}
          >
            <Icon icon={copied ? Tick02Icon : Copy01Icon} size="s" />
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      )}
      {waiting.map((w) => (
        <Row
          key={w.id}
          label={tx("{0} invite, not used yet", { "0": ROLE_NAMES[w.role] ?? w.role })}
          help={tx("Runs out on {0}", { "0": new Date(w.expires_at * 1000).toLocaleDateString(language) })}
        >
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => {
              api
                .teamCancelInvite(teamId, w.id)
                .then(load)
                .catch((e) => setProblem(say(e)));
            }}
          >
            {tx("Cancel")}</button>
        </Row>
      ))}
      <Problem text={problem} />
    </Section>
  );
}

function SharedInboxes({
  teamId,
  accounts,
  canManage,
  onChanged,
}: {
  teamId: number;
  accounts: Account[];
  canManage: boolean;
  onChanged: () => void;
}) {
  const [confirm, setConfirm] = useState<{ account: Account; sharing: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const apply = async () => {
    if (!confirm) return;
    const { account, sharing } = confirm;
    setConfirm(null);
    setBusy(true);
    setProblem(null);
    try {
      await (sharing ? api.teamShareInbox(teamId, account.id) : api.teamUnshareInbox(account.id));
      onChanged();
    } catch (e) {
      setProblem(say(e));
    }
    setBusy(false);
  };

  // One team to an inbox, so another team's are left out.
  const shown = accounts.filter((a) => (a.prefs.team_shared ? a.prefs.team_id === teamId : canManage));

  return (
    <Section
      title={tx("Shared inboxes")}
      note={tx("Everyone in the team gets these, without seeing the password.")}
    >
      {shown.length === 0 && <div className="group-row muted">{tx("Nothing shared yet.")}</div>}
      {shown.map((a) => (
        <div key={a.id} className="group-row" data-inbox={a.email}>
          <Avatar name={a.email} hue={a.prefs.color ?? undefined} />
          <span className="grow truncate-1 strong">
            {a.email}
            {a.prefs.team_shared && <span className="pill">shared</span>}
          </span>
          {canManage && (
            <button
              className="btn btn-secondary btn-sm"
              disabled={busy}
              onClick={() => setConfirm({ account: a, sharing: !a.prefs.team_shared })}
            >
              {a.prefs.team_shared ? tx("Stop sharing") : tx("Share with team")}
            </button>
          )}
        </div>
      ))}
      <Problem text={problem} />
      {confirm && (
        <Confirm
          title={confirm.sharing ? `Share ${confirm.account.email}?` : `Stop sharing ${confirm.account.email}?`}
          confirm={confirm.sharing ? tx("Yes, share it") : tx("Yes, stop sharing")}
          keep={tx("Go back")}
          danger={!confirm.sharing}
          onKeep={() => setConfirm(null)}
          onConfirm={() => void apply()}
        >
          {confirm.sharing
            ? tx("Everyone in the team will be able to read this inbox and send from it, on all their devices.")
            : tx("It leaves everyone else's devices, and stays connected only on yours. Their devices have held the password, so change it if you need to be sure.")}
        </Confirm>
      )}
    </Section>
  );
}

function Templates({ teamId }: { teamId: number }) {
  const [list, setList] = useState<TeamTemplate[]>([]);
  const [editing, setEditing] = useState<Partial<TeamTemplate> | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const load = useCallback(() => {
    api
      .teamTemplates(teamId)
      .then(setList)
      .catch(() => {});
  }, [teamId]);
  useEffect(load, [load]);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    // A second click while it saves would add the same template twice.
    if (!editing || saving) return;
    setSaving(true);
    setProblem(null);
    try {
      await api.teamSaveTemplate(teamId, editing.id ?? null, editing.name ?? "", editing.body ?? "", editing.version ?? 0);
      setEditing(null);
      load();
    } catch (err) {
      setProblem(say(err));
      // A teammate saved first. Saving again, having been told, replaces theirs.
      const fresh = await api.teamTemplates(teamId).catch(() => null);
      if (fresh) {
        setList(fresh);
        const theirs = fresh.find((t) => t.id === editing.id);
        setEditing({ ...editing, id: theirs ? editing.id : undefined, version: theirs?.version ?? 0 });
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <Section title={tx("Templates")} note={tx("Ready-made replies for everyone in the team.")}>
      {list.map((t) => (
        <div key={t.id} className="group-row" data-template={t.name}>
          <span className="grow truncate-1 strong">{t.name}</span>
          <IconButton icon={Edit02Icon} label={`Edit ${t.name}`} onClick={() => setEditing(t)} />
          <IconButton
            icon={Delete02Icon}
            label={`Delete ${t.name}`}
            onClick={() => {
              api
                .teamDeleteTemplate(teamId, t.id, t.version)
                .then(load)
                .catch((e) => {
                  setProblem(say(e));
                  load();
                });
            }}
          />
        </div>
      ))}
      <div className="group-row">
        <button className="btn btn-secondary btn-sm" onClick={() => setEditing({ name: "", body: "" })}>
          <Icon icon={Add01Icon} size="s" />
          {tx("New template")}</button>
      </div>
      {!editing && <Problem text={problem} />}
      {editing && (
        <Dialog
          title={editing.id ? tx("Edit template") : tx("New template")}
          subtitle={tx("Anyone in the team can use it when writing an email.")}
          width="m"
          onClose={() => setEditing(null)}
          footer={
            <>
              <button type="button" className="btn btn-ghost" onClick={() => setEditing(null)}>
                {tx("Cancel")}</button>
              <button
                type="submit"
                form="team-template"
                className="btn btn-primary"
                disabled={saving || !editing.name?.trim() || !editing.body?.trim()}
              >
                {saving && <Spinner size="xs" />}
                {tx("Save template")}</button>
            </>
          }
        >
          <form id="team-template" className="stack" onSubmit={save}>
            <label className="field">
              <span className="field-label">{tx("Name")}</span>
              <input
                autoFocus
                placeholder={tx("For example: Thanks for writing in")}
                value={editing.name ?? ""}
                onChange={(e) => setEditing({ ...editing, name: e.target.value })}
              />
            </label>
            <label className="field">
              <span className="field-label">{tx("Message")}</span>
              <textarea
                className="input template-body"
                rows={7}
                placeholder={tx("Write the message your team can reuse")}
                value={editing.body ?? ""}
                onChange={(e) => setEditing({ ...editing, body: e.target.value })}
              />
            </label>
            <Problem text={problem} />
          </form>
        </Dialog>
      )}
    </Section>
  );
}

function LeaveTeam({
  teamId,
  isOwner,
  onDone,
  onProblem,
}: {
  teamId: number;
  isOwner: boolean;
  onDone: () => void;
  onProblem: (text: string | null) => void;
}) {
  const [confirming, setConfirming] = useState(false);

  const go = async () => {
    setConfirming(false);
    onProblem(null);
    try {
      await (isOwner ? api.teamDelete(teamId) : api.teamLeave(teamId));
      onDone();
    } catch (e) {
      onProblem(say(e));
    }
  };

  return (
    <Section title={isOwner ? tx("Delete this team") : tx("Leave this team")}>
      <Row
        label={isOwner ? tx("Delete this team") : tx("Leave this team")}
        help={
          isOwner
            ? tx("Removes the team for everyone, with its notes and templates. Shared inboxes leave everyone's devices. Email stays with your provider.")
            : tx("Shared inboxes leave your devices. Email stays with your provider.")
        }
      >
        <button className="btn btn-secondary btn-sm" onClick={() => setConfirming(true)}>
          {isOwner ? tx("Delete team") : tx("Leave team")}
        </button>
      </Row>
      {confirming && (
        <Confirm
          title={isOwner ? tx("Delete this team?") : tx("Leave this team?")}
          confirm={isOwner ? tx("Yes, delete it") : tx("Yes, leave")}
          keep={tx("Stay")}
          danger
          onKeep={() => setConfirming(false)}
          onConfirm={() => void go()}
        >
          {isOwner
            ? tx("This can't be undone. Everyone loses the shared inboxes, notes and templates. To keep an inbox yourself, stop sharing it first.")
            : tx("You'll need a new invite code to come back.")}
        </Confirm>
      )}
    </Section>
  );
}
