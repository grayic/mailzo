import { useState } from "react";
import { Copy01Icon, Key01Icon, Tick02Icon } from "../icons";
import { Icon } from "./ui";
import { tx } from "../i18n";

/** Shows a recovery key. It is never stored, so this is the only chance to save it. */
export function RecoveryKey({ value, onDone }: { value: string; onDone: () => void }) {
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="welcome">
      <div className="welcome-drag" data-tauri-drag-region />
      <div className="welcome-inner">
        <div className="welcome-card welcome-card-wide stack-l">
          <div>
            <div className="empty-icon">
              <Icon icon={Key01Icon} size="l" />
            </div>
            <h1 className="welcome-title">{tx("Save your recovery key")}</h1>
            <p className="welcome-text">
              {tx("Your inboxes and settings are locked so only your own devices can open them. Not even Mailzo can. If you ever lose every device, this key is the only way to unlock them, so save it in your password manager. We can't show it again.")}</p>
          </div>
          <div className="stack">
            <div aria-label={tx("Recovery key")} className="selectable key-box">
              {value}
            </div>
            <button className="btn btn-secondary btn-block" onClick={() => void copy()}>
              <Icon icon={copied ? Tick02Icon : Copy01Icon} size="s" />
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
          <p className="fine">{tx("Your email itself is always safe with your email provider.")}</p>
          <label className="check-row">
            <input
              type="checkbox"
              className="check"
              checked={saved}
              onChange={(e) => setSaved(e.target.checked)}
            />
            <span>{tx("I've saved it somewhere safe")}</span>
          </label>
          <button className="btn btn-primary btn-lg btn-block" disabled={!saved} onClick={onDone}>
            {tx("Continue")}</button>
        </div>
      </div>
    </div>
  );
}
