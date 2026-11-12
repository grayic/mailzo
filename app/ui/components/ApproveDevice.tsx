import { useState } from "react";
import { ApiError, api, type PendingDevice } from "../api";
import { LaptopIcon } from "../icons";
import { Dialog, Icon, Problem } from "./ui";
import { tx } from "../i18n";

/**
 * Another of the person's devices asks to join. One tap allows it, after they see the
 * number here matches the one on the new device.
 */
export function ApproveDevice({ device, onDone }: { device: PendingDevice; onDone: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const allow = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.approveDevice(device.id, device.number);
      onDone();
    } catch (err) {
      const refused = err instanceof ApiError && err.message.toLowerCase().includes("refused");
      setError(err instanceof ApiError ? err.message : String(err));
      setBusy(false);
      if (refused) setTimeout(onDone, 2500);
    }
  };

  const refuse = async () => {
    setBusy(true);
    try {
      await api.rejectDevice(device.id);
      onDone();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
      setBusy(false);
    }
  };

  return (
    <Dialog
      label={tx("Allow a new device")}
      title={`Allow ${device.name} to use your inboxes?`}
      width="s"
      footer={
        <>
          <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => void refuse()}>
            {tx("Not me")}</button>
          <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void allow()}>
            {tx("Allow")}</button>
        </>
      }
    >
      <div className="stack">
        <div className="group">
          <div className="group-row">
            <span className="group-icon">
              <Icon icon={LaptopIcon} size="s" />
            </span>
            <span className="strong truncate-1">{device.name}</span>
          </div>
        </div>
        <p className="body-text">{tx("Check this number matches the one on that device, then tap Allow.")}</p>
        <div aria-label={tx("Number to match")} className="selectable number-box">
          {device.number}
        </div>
        <Problem text={error} />
        <p className="fine">
          {tx("If you didn't just sign in on another device, tap Not me. Then change your email's password, because someone may be able to read it.")}</p>
      </div>
    </Dialog>
  );
}
