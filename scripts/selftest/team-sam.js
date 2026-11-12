guarded(async () => {
  const code = await shareWait("teamCode", 90000);
  await waitFor(() => pageHas("Welcome to Mailzo"), 20000);
  await press(() => buttonWith("Already have an account"));
  check("a teammate signs up", await signInAs("sam@team.test", "codeSam"));

  // With no inbox of their own, the connect screen is where they start.
  await waitFor(() => pageHas("Connect"), 20000);
  (await waitFor(() => buttonWith("Join a team"), 15000)).click();
  (await waitFor(() => buttonWith("I have an invite code"))).click();
  setValue(await waitFor(() => labelled("Invite code")), code);
  await sleep(50);
  await press(() => buttonWith("Join team"));
  check("an invite code gets them in", await waitFor(() => document.querySelector(".settings-page h1")?.innerText === "Grayic", 15000), document.querySelector(".settings-page")?.innerText.slice(0, 200));
  await closeSettings();
  check("the shared inbox arrives by itself", await waitFor(() => folderButton("Inbox"), 40000));
  const list = await invoke("call", { cmd: "list_accounts", args: {} });
  check("it's marked as the team's", list[0]?.prefs.team_shared === true);

  await shareWait("templateReady", 60000);
  (await waitFor(() => folderButton("Inbox"))).click();
  check("the teammate has the team's views", await waitFor(() => tabNamedT("Unassigned")));
  check("mail reaches the teammate too", !!(await untilTeamRow()));

  await shareWait("bossRead", 60000);
  refreshMail();
  await sleep(2000);
  check("someone else reading leaves it unread here", !!teamRow()?.querySelector(".unread-bar"));
  await press(() => teamRow());
  await waitFor(() => holder() === "Assign", 15000);
  await shareSet("samReady", "1");

  const at = Number(await shareWait("claimAt", 30000));
  await waitUntil(at);
  // Take this is in the team chip's menu.
  document.querySelector(".team-assign")?.click();
  // Whoever is slower may find it already taken, with no Take this left.
  (await menuItem("Take this"))?.click();
  await waitFor(() => holder() && holder() !== "Assign", 15000);
  // If it was already gone, a menu is still open (the Assign one, or the one menuItem fell back on): close it.
  await waitFor(() => {
    if (!document.querySelector('[role="menu"]')) return true;
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    return false;
  });
  const mine = holder();
  await shareSet("samSawHolder", mine);
  await shareWait("bossSawHolder");

  // A note, with a mention. The box stays closed until someone wants to write.
  const addFirst = await waitFor(() => buttonWith("Add a team note"));
  check("with no notes there's one button to add one", !!addFirst && !labelled("Write a note for your team"));
  await press(() => addFirst);
  check("it opens the note box", !!(await waitFor(() => labelled("Write a note for your team"))));
  labelled("Write a note for your team").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  check("Escape closes an empty note box", await waitFor(() => !labelled("Write a note for your team") && buttonWith("Add a team note")));
  await press(() => buttonWith("Add a team note"));
  const area = await waitFor(() => labelled("Write a note for your team"));
  setArea(area, "@bo");
  check("typing @ suggests teammates", await waitFor(() => document.querySelector('[aria-label="Mention someone"] button')?.innerText.includes("boss@team.test")));
  await press(() => document.querySelector('[aria-label="Mention someone"] button'));
  await waitFor(() => labelled("Write a note for your team").value === "@boss@team.test ");
  const noteBox = labelled("Write a note for your team");
  const oneLine = noteBox.offsetHeight;
  setArea(noteBox, "One\nTwo\nThree");
  check("the note box grows with what's written", await waitFor(() => labelled("Write a note for your team").offsetHeight >= oneLine * 2));
  labelled("Write a note for your team").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true, cancelable: true }));
  await sleep(200);
  check("Shift+Enter doesn't post the note", labelled("Write a note for your team").value === "One\nTwo\nThree");
  setArea(labelled("Write a note for your team"), "@boss@team.test Please look at this");
  await sleep(50);
  labelled("Write a note for your team").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  check("my note is saved", await waitFor(() => document.querySelector(".team-note-text")?.innerText.includes("Please look at this"), 15000));
  check("the box closes after posting", await waitFor(() => !labelled("Write a note for your team") && buttonWith("Add a note")));
  await press(() => buttonWith("Add a note"));
  check("Add a note opens it again", !!(await waitFor(() => labelled("Write a note for your team"))));
  (await waitFor(() => [...document.querySelectorAll(".team-note-hint button")].find((b) => b.innerText.trim() === "Cancel"))).click();
  check("Cancel closes it", await waitFor(() => !labelled("Write a note for your team") && buttonWith("Add a note")));

  // Replying inside the email, with a template and no accidents.
  await press(() => document.querySelector(".quick-reply"));
  check("the reply opens in the email", await waitFor(() => document.querySelector(".composer-inline .ProseMirror")));
  await press(() => labelled("Templates"));
  (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes("Thanks")))).click();
  check("a template goes into the reply", await waitFor(() => document.querySelector(".composer-inline .ProseMirror")?.innerText.includes("Thanks for writing in.")));
  document.activeElement?.blur();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace", bubbles: true }));
  await sleep(800);
  check("Backspace doesn't bin the email while replying", !!teamRow() && !!document.querySelector(".composer-inline"));

  await shareWait("bossSent", 60000);
  const before = await outboxCount();
  await press(() => [...document.querySelectorAll(".composer-inline button")].find((b) => b.innerText.includes("Send")));
  const stopped = await waitFor(() => document.querySelector(".composer-problem")?.innerText.includes("boss@team.test replied while you were writing"), 30000);
  check("a second reply is stopped, naming who answered", !!stopped, document.querySelector(".composer-problem")?.innerText);
  check("nothing extra went out", (await outboxCount()) === before);
  (await waitFor(() => buttonWith("Send anyway"))).click();
  check("sending anyway works", !!(await nextSent(before)));
  await shareSet("samDone", "1");

  await shareWait("markedDone", 60000);
  check("done reaches me by itself", await waitFor(() => !teamRow(), 20000));
  await shareSet("samSawDone", "1");

  await shareWait("removed", 60000);
  check("being removed takes the inbox away", await waitFor(() => pageHas("Connect") && !folderButton("Inbox"), 30000));
  const status = await invoke("call", { cmd: "team_status", args: {} });
  check("and the team", status.state === "none", status.state);
  await shareSet("samGone", "1");
});
