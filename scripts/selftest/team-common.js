// Two people in two real windows, working on one shared inbox at the same moment.
const teamRow = () => [...document.querySelectorAll(".thread")].find((r) => r.innerText.includes("Team question"));
const refreshMail = () => document.querySelector('[aria-label="Refresh mail"]')?.click();
const setArea = (area, value) => {
  const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
  set.call(area, value);
  area.dispatchEvent(new Event("input", { bubbles: true }));
};
const holder = () => document.querySelector(".team-assign .truncate-1")?.innerText.trim();
const signInAs = async (email, codeName) => {
  await waitFor(() => buttonWith("Email me a code"), 20000);
  const input = document.querySelector('input[type="email"]');
  if (!input) throw new Error(`no email box: ${document.body.innerText.slice(0, 300)}`);
  setValue(input, email);
  await sleep(50);
  await press(() => buttonWith("Email me a code"));
  const box = await waitFor(() => document.querySelector('input[autocomplete="one-time-code"]'), 20000);
  if (!box) throw new Error(`no code box: ${document.body.innerText.slice(0, 300)}`);
  const code = await shareWait(codeName, 20000);
  setValue(box, code);
  await sleep(50);
  await press(() => buttonWith("Continue"));
  const key = await waitFor(() => dialog("Recovery key"), 20000);
  await press(() => document.querySelector('input[type="checkbox"]'));
  await sleep(50);
  await press(() => buttonWith("Continue"));
  return !!key;
};
const untilTeamRow = async () => {
  const end = Date.now() + 90000;
  while (Date.now() < end) {
    if (teamRow()) return teamRow();
    refreshMail();
    await sleep(2000);
  }
  return null;
};
const waitUntil = async (at) => {
  while (Date.now() < at) await sleep(5);
};
const tabNamedT = (name) => [...document.querySelectorAll('[role="tab"]')].find((t) => t.innerText.startsWith(name));
