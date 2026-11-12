import type { IconSvgElement } from "@hugeicons/react";
import { ReactNode } from "react";
import { ArrowDown01Icon, Tick02Icon } from "../../icons";
import { useSettings } from "../../settings";
import { Icon, Menu, MenuItem } from "../ui";

export function Page({
  title,
  intro,
  action,
  children,
}: {
  title: string;
  intro?: string;
  /** The main thing to do here, beside the title. */
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="settings-page-inner">
      <div className="settings-head">
        <div className="grow">
          <h1 className="settings-title">{title}</h1>
          {intro && <p className="settings-intro">{intro}</p>}
        </div>
        {action}
      </div>
      <div className="stack-l">{children}</div>
    </div>
  );
}

/** What a page shows before anything has been added to it. */
export function EmptyPage({ title, text }: { title: string; text: string }) {
  return (
    <div className="settings-empty">
      <div className="strong">{title}</div>
      <div className="muted">{text}</div>
    </div>
  );
}

export function Section({ title, note, children }: { title?: ReactNode; note?: ReactNode; children: ReactNode }) {
  return (
    <section>
      {title && <div className="section-title">{title}</div>}
      <div className="group">{children}</div>
      {note && <p className="fine note">{note}</p>}
    </section>
  );
}

export function Row({
  label,
  help,
  icon,
  children,
  stacked,
}: {
  label: ReactNode;
  help?: ReactNode;
  icon?: IconSvgElement;
  children?: ReactNode;
  /** The control sits under the label, for ones that need the full width. */
  stacked?: boolean;
}) {
  return (
    <div className={`group-row ${stacked ? "group-row-stacked" : ""}`}>
      {icon && (
        <span className="group-icon">
          <Icon icon={icon} size="s" />
        </span>
      )}
      <div className="grow">
        <div className="row-label">{label}</div>
        {help && <div className="row-help">{help}</div>}
      </div>
      {children}
    </div>
  );
}

export function Toggle({
  label,
  on,
  disabled,
  onChange,
}: {
  label: string;
  on: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <input
      type="checkbox"
      role="switch"
      className="toggle"
      aria-label={label}
      checked={on}
      disabled={disabled}
      onChange={(e) => onChange(e.target.checked)}
    />
  );
}

export type Option = { value: string; label: string };

/** A short list of choices, all visible at once. */
export function Segmented({
  label,
  options,
  value,
  onChange,
  disabled,
}: {
  label: string;
  options: Option[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}) {
  return (
    <div role="radiogroup" aria-label={label} aria-disabled={disabled || undefined} className="segmented">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          disabled={disabled}
          className="segment"
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** A longer list of choices, behind a button. */
export function Choice({
  label,
  options,
  value,
  onChange,
  className = "choice",
}: {
  label: string;
  options: Option[];
  value: string;
  onChange: (value: string) => void;
  /** `list-filter` for the small pill in a list's header. */
  className?: string;
}) {
  const current = options.find((o) => o.value === value);
  return (
    <Menu
      label={label}
      align="end"
      trigger={(props) => (
        <button {...props} type="button" className={className} aria-label={label} title={current?.label}>
          <span className="truncate-1">{current?.label ?? "Choose"}</span>
          <Icon icon={ArrowDown01Icon} size="xs" />
        </button>
      )}
    >
      {(close) =>
        options.map((o) => (
          <MenuItem
            key={o.value}
            trailing={o.value === value ? <Icon icon={Tick02Icon} size="s" /> : undefined}
            onClick={() => {
              close();
              onChange(o.value);
            }}
          >
            {o.label}
          </MenuItem>
        ))
      }
    </Menu>
  );
}

/** A row wired straight to a saved on/off setting. */
export function SettingToggle({
  setting,
  label,
  help,
  icon,
  disabled,
  onChanged,
}: {
  setting: string;
  label: string;
  help?: ReactNode;
  icon?: IconSvgElement;
  disabled?: boolean;
  onChanged?: (on: boolean) => void;
}) {
  const { settings, change } = useSettings();
  return (
    <Row label={label} help={help} icon={icon}>
      <Toggle
        label={label}
        on={settings[setting] === "on"}
        disabled={disabled}
        onChange={(on) => {
          change(setting, on ? "on" : "off");
          onChanged?.(on);
        }}
      />
    </Row>
  );
}

/** A row wired straight to a saved setting with a few choices. */
export function SettingChoice({
  setting,
  label,
  help,
  options,
  menu,
  disabled,
}: {
  setting: string;
  label: string;
  help?: ReactNode;
  options: Option[];
  /** Behind a button, for lists too long to show at once. */
  menu?: boolean;
  /** Only the side-by-side choices can be switched off. */
  disabled?: boolean;
}) {
  const { settings, change } = useSettings();
  return (
    <Row label={label} help={help}>
      {menu ? (
        <Choice label={label} options={options} value={settings[setting] ?? ""} onChange={(v) => change(setting, v)} />
      ) : (
        <Segmented
          label={label}
          options={options}
          value={settings[setting] ?? ""}
          disabled={disabled}
          onChange={(v) => change(setting, v)}
        />
      )}
    </Row>
  );
}
