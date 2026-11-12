// Helpers for driving the real screens inside the Mac app.
const results = {};
const check = (name, ok, detail) => {
  results[name] = ok ? "pass" : `FAIL ${detail === undefined ? "" : JSON.stringify(detail)}`;
  // So a run that gets stuck can say how far it got.
  window.__TAURI_INTERNALS__?.invoke("selftest_share", { name: "progress", value: `${Object.keys(results).length}: ${name}: ${results[name]}` }).catch?.(() => {});
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// Waiting only costs time when something has really gone wrong, and a busy Mac can be very slow.
const PATIENCE_MS = 30000;
const waitFor = async (fn, ms = PATIENCE_MS) => {
  const end = Date.now() + Math.max(ms, PATIENCE_MS);
  while (Date.now() < end) {
    try { const v = fn(); if (v) return v; } catch (_) {}
    await sleep(100);
  }
  return null;
};
// Clicks it once it's there and can be pressed, and names it if it never is.
const press = async (find) => {
  const found = await waitFor(() => {
    const el = find();
    return el && !el.disabled ? el : null;
  });
  if (!found) throw new Error(`never appeared: ${find}`);
  found.click();
};
// Someone working in another app takes the focus away, which would close menus part way through.
window.addEventListener("blur", (e) => e.target === window && e.stopImmediatePropagation(), true);
const pageHas = (t) => document.body.innerText.includes(t);
// An exact match comes first, so "Continue" doesn't find "Continue with Google".
const buttonWith = (t) => {
  const buttons = [...document.querySelectorAll("button:not(.copy-address)")];
  return (
    buttons.find((b) => b.innerText.trim() === t) ??
    buttons.find((b) => `${b.innerText} ${b.getAttribute("aria-label") ?? ""}`.includes(t))
  );
};
// An item in whichever menu is open.
const menuItem = (t) => waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes(t)));
// Your account lives in Settings, under Profile.
const openAccount = () => openSettings("Profile");
// With no account yet, Profile offers one for other devices.
const startAccount = async () => {
  await openAccount();
  await press(() => buttonWith("Set up a free account"));
};
/** From inside the app, with no account yet: starts one with the connected inbox's email. */
const setUpAccount = async () => {
  await startAccount();
  (await waitFor(() => buttonWith("Continue with this email"), 20000)).click();
};
// The filter's menu can close by itself if the list is still changing, so this reopens it.
const flipUnreadFilter = () =>
  waitFor(() => {
    if (!document.querySelector('[role="menu"]')) document.querySelector(".list-filter").click();
    const item = [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes("Unread only"));
    item?.click();
    return item;
  });
const openSettings = async (page) => {
  if (!document.querySelector(".settings")) window.dispatchEvent(new KeyboardEvent("keydown", { key: ",", metaKey: true }));
  const nav = await waitFor(() => [...document.querySelectorAll(".settings-nav .nav-item")].find((b) => b.querySelector(".truncate-1")?.innerText.trim() === page));
  await press(() => nav);
  await sleep(100);
};
const closeSettings = async () => {
  await press(() => document.querySelector('.settings-bar [aria-label="Close"]'));
  await waitFor(() => !document.querySelector(".settings"));
};
const inSettings = (t) => [...document.querySelectorAll(".settings button")].find((b) => `${b.innerText} ${b.getAttribute("aria-label") ?? ""}`.includes(t));
const setValue = (input, value) => {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
  set.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
const finish = () =>
  window.__TAURI_INTERNALS__.invoke("selftest_report", { report: JSON.stringify(results, null, 2) });
const guarded = (fn) => fn().catch((e) => { results.crashed = String(e); }).finally(finish);
// Two copies of the app pass notes through the self-test's shared folder.
const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
const shareSet = (name, value) => invoke("selftest_share", { name, value: String(value) });
const shareGet = (name) => invoke("selftest_share", { name, value: null });
const shareWait = async (name, ms = 120000, unlessEqual) => {
  const end = Date.now() + Math.max(ms, 120000);
  while (Date.now() < end) {
    const v = await shareGet(name);
    if (v && v !== unlessEqual) return v;
    await sleep(150);
  }
  return null;
};
const dialog = (label) => document.querySelector(`[aria-label="${label}"]`);
const signInWith = async (email) => {
  const previous = await shareGet("code");
  const input = await waitFor(() => document.querySelector('input[type="email"]'), 20000);
  setValue(input, email);
  await sleep(50);
  await press(() => buttonWith("Email me a code"));
  return { code: await shareWait("code", 20000, previous), input: await waitFor(() => document.querySelector('input[autocomplete="one-time-code"]')) };
};
const connectDemoInbox = async (password) => {
  const email = await waitFor(() => pageHas("Welcome to Mailzo") && document.querySelector('input[type="email"]'));
  setValue(email, "demo@inbox.test");
  await sleep(50);
  await press(() => buttonWith("Continue"));
  // Nothing is found for this made-up address, so the server boxes open by themselves.
  await waitFor(() => document.querySelectorAll("input").length >= 4);
  const inputs = document.querySelectorAll("input");
  setValue(inputs[1], password);
  setValue(inputs[2], "127.0.0.1");
  setValue(inputs[3], "1143");
  await sleep(50);
  await press(() => buttonWith("Connect inbox"));
};
