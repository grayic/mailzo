// Reopened with the mail server and account server both switched off.
guarded(async () => {
  check("stays signed in, with mail, after a restart", await waitFor(() => pageHas("Weekly Digest") && pageHas("Maya Chen")));
  check("opens on the inbox, as chosen in Settings", await waitFor(() => document.querySelector(".list-title")?.innerText === "Inbox"));
  // A refresh failing once on its own stays quiet; pressing Refresh says what's wrong at once.
  await press(() => document.querySelector('[aria-label="Refresh mail"]'));
  check("failed refresh shows a notice, not a blank app", await waitFor(() => /Couldn't (refresh|reach)/.test(document.querySelector('[role="status"]')?.innerText ?? "")), document.querySelector('[role="status"]')?.innerText);
  await press(() => buttonWith("Maya Chen"));
  check("saved mail reads offline", await waitFor(() => document.querySelectorAll("article").length === 3 && pageHas("Final version goes out on Tuesday")));
  await press(() => buttonWith("Weekly Digest"));
  check("saved html mail reads offline", !!(await waitFor(() => document.querySelector("iframe"))));

  await openAccount();
  (await waitFor(() => buttonWith("Sign out of this device"))).click();
  (await waitFor(() => buttonWith("Yes, sign out"))).click();
  check("signing out offline explains itself and keeps the mail", await waitFor(() => pageHas("Could not reach the account server") && pageHas("Weekly Digest")));
  await closeSettings();

  await openSettings("demo@inbox.test");
  (await waitFor(() => buttonWith("Disconnect this inbox"))).click();
  (await waitFor(() => buttonWith("Yes, disconnect"))).click();
  check("disconnecting returns to the connect screen", await waitFor(() => pageHas("Connect an inbox") && document.querySelector('input[type="email"]')));
});
