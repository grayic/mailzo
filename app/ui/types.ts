export interface Signature {
  id: string;
  name: string;
  html: string;
}

export interface AccountPrefs {
  team_shared?: boolean;
  team_id?: number;
  shared_ai_allowed?: boolean;
  label: string;
  color: number | null;
  in_all_inbox: boolean;
  signatures: Signature[];
  signature_new: string | null;
  signature_reply: string | null;
  aliases: Addr[];
  reply_from: string;
  alerts: string;
  folder_roles: Record<string, string>;
  default_for_new: boolean;
  ai_allowed: boolean;
  ai_skip_folders: string[];
  /** What the person tells AI about themselves and this inbox, for writing replies. */
  about?: string;
}

export interface Account {
  id: number;
  /** What every device knows this inbox by. */
  sync_id: string;
  email: string;
  display_name: string;
  imap_host: string;
  imap_port: number;
  security: "tls" | "plain";
  username: string;
  smtp_host: string;
  smtp_port: number;
  smtp_security: "tls" | "starttls" | "plain";
  signature: string;
  /** `password`, or the provider signed in with through the browser. */
  auth: string;
  prefs: AccountPrefs;
}

export interface Folder {
  id: number;
  account_id: number;
  path: string;
  name: string;
  role: string;
  total: number;
  unread: number;
  last_synced_at: number | null;
}

export interface SearchFilters {
  words: string[];
  similar: string[];
  from: string[];
  to: string[];
  after: number | null;
  before: number | null;
  has_attachment: boolean;
  unread: boolean;
  starred: boolean;
  folder: string | null;
  include_bin: boolean;
  /** Only this inbox. */
  account?: number | null;
}

export interface SearchReply {
  threads: ThreadSummary[];
  in_bin: number;
  filters: SearchFilters;
  extra_ids?: number[];
  unreached?: string[];
}

export interface ThreadSummary {
  /** A sign-in or verification code the email gives, to copy in one click. */
  code?: string;
  needs_you: boolean;
  why: string | null;
  ai_line: string | null;
  due_at: number | null;
  thread_id: string;
  account_id: number;
  account_email: string;
  lead_address: string;
  folder_id: number;
  subject: string;
  participants: string[];
  snippet: string;
  last_date: number;
  message_count: number;
  unread_count: number;
  flagged: boolean;
  has_attachments: boolean;
  /** 0 is fine, 1 is worth a warning, 2 looks like spam. */
  risk: number;
  label: string | null;
  team?: ThreadTeam;
  pinned: boolean;
  muted: boolean;
  snooze_until: number | null;
  returned_at: number | null;
  follow_due: boolean;
  follow_at: number | null;
  /** In a search, what it's looking for is right there in the subject. */
  best_match?: boolean;
  /** The person's own tags on it. */
  tags?: ThreadTag[];
}

/** A label the person made, such as Paid. */
export interface Tag {
  id: number;
  name: string;
  colour: string;
  /** How many conversations have it. */
  count: number;
}

/** A tag on one conversation, with an amount if one was given. */
export interface ThreadTag {
  tag_id: number;
  amount?: number | null;
}

/** Who has an email in an inbox shared with a team. */
export interface ThreadTeam {
  assignee: number | null;
  assignee_email: string | null;
  mine: boolean;
  done: boolean;
  comments: number;
  viewing: string[];
  replying: string[];
}

export interface Addr {
  name: string;
  address: string;
}

export interface Attachment {
  name: string;
  mime: string;
  size: number;
  part: number | null;
  inline: boolean;
}

/** Everything shared in one conversation. */
export interface SharedStuff {
  files: { message_id: number; part: number; name: string; mime: string; size: number; date: number; from: string }[];
  links: { url: string; text: string; host: string; message_id: number; date: number; from: string; risky: boolean }[];
}

export interface DraftAttachment {
  id: string;
  name: string;
  mime: string;
  size: number;
}

export interface Draft {
  id: number;
  account_id: number;
  mode: "new" | "reply" | "reply_all" | "forward";
  parent_message_id: number | null;
  from_address: string;
  to: Addr[];
  cc: Addr[];
  bcc: Addr[];
  subject: string;
  html: string;
  text: string;
  quoted_html: string;
  quoted_text: string;
  attachments: DraftAttachment[];
  updated_at: number;
  send_at: number | null;
  send_problem: string | null;
  remind_days: number | null;
  /** Written by an outside AI tool, for the person to check. */
  made_by_ai: boolean;
  /** Sent encrypted with PGP, to every recipient's key. */
  encrypt?: boolean;
  /** Signed with this inbox's PGP key. */
  sign?: boolean;
  /** Replies to or forwards an encrypted email. Never saved to the server's Drafts. */
  from_encrypted?: boolean;
  /** The person confirmed sending it unencrypted anyway. */
  plain_ok?: boolean;
}

export interface Contact {
  name: string;
  address: string;
}

/** What encryption did to an email: shown above it. Null for ordinary mail. */
export interface PgpInfo {
  encrypted: boolean;
  signed: boolean;
  verified: boolean;
  signer: string | null;
  problem: string | null;
  /** Signed with a key that only came from the sender's earlier email. */
  signer_unconfirmed?: boolean;
  /** Signed with a different key from the one kept for the sender. */
  key_changed?: boolean;
  /** Only part of the text is inside the signed or encrypted part. */
  partial?: boolean;
}

export interface Body {
  text: string | null;
  html: string | null;
  attachments: Attachment[];
  pgp?: PgpInfo | null;
}

export interface Message {
  id: number;
  account_id: number;
  thread_id: string;
  subject: string;
  from: Addr;
  reply_to: Addr[];
  to: Addr[];
  cc: Addr[];
  bcc: Addr[];
  date: number;
  seen: boolean;
  flagged: boolean;
  folder_id: number;
  body: Body | null;
  risk: number;
  warnings: string[];
  can_unsubscribe: boolean;
  /** The day it asks for something by, found in its words (the start of that day, UTC). */
  due_at: number | null;
}

export interface SyncStats {
  folders: number;
  new_messages: number;
  removed_messages: number;
  bodies_fetched: number;
  already_running: boolean;
  errors: string[];
  older_saved: number;
  more_to_download: boolean;
  /** Emails read, unread, starred or unstarred elsewhere. */
  flags_changed: number;
  /** New mail checked and spam moved. */
  filtered: number;
}

/** What a reply opens with: words already written, or a note for AI to write from. */
export type ReplyStart = { text: string } | { note: string };
