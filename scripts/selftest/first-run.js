// The first device: sets up the account, then approves a second device.
guarded(async () => {
  check("connecting an inbox comes first, with no account", await waitFor(() => pageHas("Welcome to Mailzo")));
  check("no sign-in has happened yet", (await invoke("call", { cmd: "auth_status", args: {} })).state === "signed_out");

  await connectDemoInbox("not-the-password");
  check("wrong inbox password shows a clear message", await waitFor(() => pageHas("rejected this email address")));
  await press(() => buttonWith("Show password"));
  check("the password can be shown", await waitFor(() => document.querySelectorAll("input")[1].type === "text"));
  await press(() => buttonWith("Hide password"));
  check("and hidden again", await waitFor(() => document.querySelectorAll("input")[1].type === "password"));
  setValue(document.querySelectorAll("input")[1], "demo-password");
  await sleep(50);
  await press(() => buttonWith("Connect inbox"));

  check("the mail opens right away, with no account", await waitFor(() => pageHas("Maya Chen"), 30000));
  check("still no account", (await invoke("call", { cmd: "auth_status", args: {} })).state === "signed_out");
  await openAccount();
  check("Settings offers an account for other devices", !!(await waitFor(() => buttonWith("Set up a free account"))));
  await press(() => buttonWith("Set up a free account"));
  check("it explains what the account is for", await waitFor(() => pageHas("Use Mailzo on your other devices")));
  (await waitFor(() => buttonWith("Not now"))).click();
  check("not now goes back to the mail", await waitFor(() => pageHas("Maya Chen")));
  await startAccount();
  (await waitFor(() => buttonWith("Use a different email"))).click();
  check("a different email offers Google and a code", await waitFor(() => buttonWith("Continue with Google") && buttonWith("Email me a code")));
  await press(() => buttonWith("Back"));
  (await waitFor(() => buttonWith("Continue with this email"))).click();

  // The app finds the code in the connected inbox, so nothing is typed from here.
  let codeShown = false;
  const watching = setInterval(() => {
    if (document.querySelector(".code-input")?.value) codeShown = true;
  }, 20);
  const key = await waitFor(() => dialog("Recovery key"), 20000);
  clearInterval(watching);
  check("code was read from the inbox and setup began", !!key, document.body.innerText.slice(0, 300));
  check("and the code never showed on screen", !codeShown);
  check("recovery key is shown once at setup", key && /^([A-Z2-7]{4}-){7}[A-Z2-7]{4}$/.test(key.innerText.trim()), key?.innerText);
  check("can't continue before saving the key", buttonWith("Continue").disabled);
  await press(() => document.querySelector('input[type="checkbox"]'));
  await sleep(50);
  await press(() => buttonWith("Continue"));

  // Most checks want everything in one list. spamFlow tries the tabs.
  check("the inbox opens on the All tab", await waitFor(() => document.querySelector('[role="tab"][aria-selected="true"]')?.innerText.startsWith("All")));
  await openSettings("Spam");
  await press(() => dialog("Sort the inbox into tabs"));
  await closeSettings();
  await waitFor(() => !document.querySelector('[role="tab"]'));

  check("inbox connects and mail syncs", await waitFor(() => pageHas("Weekly Digest") && pageHas("Maya Chen")));
  check("unread count is right, including the code email", await waitFor(() => /Inbox\s*3/.test(document.querySelector("nav").innerText)), document.querySelector("nav").innerText);
  check("no sync error notice", !document.querySelector('[role="status"]'), document.querySelector('[role="status"]')?.innerText);

  await openSettings("Privacy");
  check("pictures show by default", dialog("Pictures in emails")?.querySelector('[aria-checked="true"]')?.innerText.includes("Show by default"));
  // The checks below are about what happens with pictures held back.
  await press(() => [...dialog("Pictures in emails").querySelectorAll('[role="radio"]')].find((b) => b.innerText.includes("Hide by default")));
  await closeSettings();

  // Hostile newsletter
  await press(() => buttonWith("Weekly Digest"));
  const frame = await waitFor(() => document.querySelector("iframe"));
  check("frame has no permission to run scripts", frame && !frame.getAttribute("sandbox").includes("allow-scripts") && frame.hasAttribute("sandbox"), frame?.getAttribute("sandbox"));
  await sleep(600);
  const doc = frame.contentDocument;
  const heading = doc?.querySelector("h2");
  check("email's own styles apply", heading && getComputedStyle(heading).color === "rgb(179, 38, 30)", heading && getComputedStyle(heading).color);
  check("frame is fitted to the email", parseInt(frame.style.height) > 100, frame.style.height);
  const img = doc?.querySelector("img");
  check("tracking pixel was taken out", ![...doc.querySelectorAll("img")].some((i) => i.getAttribute("width") === "1"));
  check("and pictures don't load until asked for", img && img.naturalWidth === 0, img?.naturalWidth);
  check("email script did not run", document.title === "Mailzo" && doc.title !== "SCRIPT RAN", [document.title, doc?.title]);
  check("javascript links are disarmed", !doc.querySelector("#bad").hasAttribute("href"));
  check("forms and redirects are removed", !doc.querySelector("form") && !doc.querySelector('meta[http-equiv="refresh"]') && !doc.querySelector("script"));
  await press(() => doc.querySelector("#web"));
  await press(() => doc.querySelector("#mail"));
  await sleep(500);
  check("the frame stays on the email after a link click", frame.contentWindow.location.href === "about:srcdoc", frame.contentWindow.location.href);
  let refused = false;
  try { await invoke("plugin:opener|open_url", { url: "file:///etc/hosts" }); } catch (_) { refused = true; }
  check("screens cannot open files or links directly", refused);

  await press(() => buttonWith("Maya Chen"));
  check("conversation shows all 3 messages across folders", await waitFor(() => document.querySelectorAll("article").length === 3));
  check("title has no Re:", document.querySelector("section h1")?.innerText === "Q4 launch plan", document.querySelector("section h1")?.innerText);

  await mailFlow();
  await allInboxFlow();
  await senderPicturesFlow();
  await multiSelectFlow();
  await rowMenuFlow();
  await settingsFlow();
  await searchFlow();
  await spamFlow();
  await laterFlow();
  await newslettersFlow();
  await codeFlow();
  await providersFlow();

  // A second device asks to join.
  await shareSet("aReady", "1");
  const number = await shareWait("number");
  check("new device shows a number", /^\d{6}$/.test(number || ""), number);
  const box = await waitFor(() => dialog("Number to match"), 30000);
  check("existing device asks to allow it", !!box);
  check("the prompt names the new device", pageHas("Laptop Test"));
  check("both devices show the same number", box?.innerText.trim() === number, [box?.innerText, number]);
  await press(() => buttonWith("Allow"));
  check("one tap allows the device", await waitFor(() => !dialog("Number to match")));

  check("second device received the inboxes", await shareWait("bHasMail", 40000) === "1");

  await openAccount();
  check("account page lists both devices", await waitFor(() => pageHas("Laptop Test") && pageHas("this device")));
  await press(() => inSettings("Remove"));
  (await waitFor(() => buttonWith("Yes, remove"))).click();
  check("removed device leaves the list", await waitFor(() => !document.querySelector(".settings").innerText.includes("Laptop Test")));
  check("removed device is signed out and erased", await shareWait("bSignedOut", 40000) === "1");
  await closeSettings();
  // The restart in offline.js checks this choice was kept.
  await invoke("call", { cmd: "set_setting", args: { key: "open_first", value: "selected" } });
});
