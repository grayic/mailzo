guarded(async () => {
  await connectDemoInbox("demo-password");
  await waitFor(() => folderButton("Inbox"), 20000);
  await startAccount();
  (await waitFor(() => buttonWith("Use a different email"), 20000)).click();
  await waitFor(() => buttonWith("Email me a code"));
  check("the owner signs up", await signInAs("boss@team.test", "codeBoss"));
  await waitFor(() => folderButton("Inbox"), 20000);

  await openSettings("Team");
  (await waitFor(() => buttonWith("Start a team"))).click();
  setValue(await waitFor(() => labelled("Team name")), "Grayic");
  await sleep(50);
  await press(() => buttonWith("Create team"));
  check("a team can be started", await waitFor(() => document.querySelector(".settings-page h1")?.innerText === "Grayic", 15000));

  (await waitFor(() => buttonWith("Share with team"))).click();
  (await waitFor(() => buttonWith("Yes, share it"))).click();
  check("an inbox can be shared", await waitFor(() => document.querySelector("[data-inbox] .pill")?.innerText === "shared", 15000));

  await press(() => buttonWith("Make an invite code"));
  const code = await waitFor(() => labelled("Invite code")?.innerText, 15000);
  check("an invite code is shown", /^([A-Z2-7]{4}-){7}[A-Z2-7]{4}$/.test(code ?? ""), code);
  await shareSet("teamCode", code);
  await shareWait("samJoined", 90000);
  await openSettings("General");
  await openSettings("Team");
  (await waitFor(() => document.querySelector('[data-team="Grayic"]'), 15000)).click();
  check("the teammate appears in People", await waitFor(() => document.querySelector('[data-member="sam@team.test"]'), 15000));

  // A template everyone can use.
  await press(() => buttonWith("New template"));
  const nameBox = await waitFor(() => document.querySelector('#team-template input'));
  setValue(nameBox, "Thanks");
  setArea(document.querySelector("#team-template textarea"), "Thanks for writing in.");
  await sleep(50);
  await press(() => buttonWith("Save"));
  check("a template can be saved", await waitFor(() => document.querySelector('[data-template="Thanks"]'), 15000));
  await closeSettings();
  await shareSet("templateReady", "1");

  (await waitFor(() => folderButton("Inbox"))).click();
  check("a shared inbox has the team's views", await waitFor(() => tabNamedT("Open") && tabNamedT("Mine") && tabNamedT("Unassigned") && tabNamedT("Done")));
  await shareSet("newmail", "1");
  check("mail reaches the shared inbox", !!(await untilTeamRow()));

  // Only one person reads it, and it stays unread for the other.
  await press(() => teamRow());
  await waitFor(() => holder() === "Assign", 15000);
  await sleep(4000);
  check("reading clears it here", await waitFor(() => !teamRow()?.querySelector(".unread-bar"), 10000));
  await shareSet("bossRead", "1");
  await shareWait("samReady");

  // Both press Take this at the same moment.
  const at = Date.now() + 2500;
  await shareSet("claimAt", at);
  await waitUntil(at);
  // Take this is in the team chip's menu.
  document.querySelector(".team-assign")?.click();
  // Whoever is slower may find it already taken, with no Take this left.
  (await menuItem("Take this"))?.click();
  check("someone ends up with it", await waitFor(() => holder() && holder() !== "Assign", 15000), holder());
  // If it was already gone, a menu is still open (the Assign one, or the one menuItem fell back on): close it.
  await waitFor(() => {
    if (!document.querySelector('[role="menu"]')) return true;
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    return false;
  });
  const mine = holder();
  await shareSet("bossSawHolder", mine);
  const samSaw = await shareWait("samSawHolder");
  check(
    "both agree who has it",
    (mine === "You" && samSaw === "boss@team.test") || (mine === "sam@team.test" && samSaw === "You"),
    [mine, samSaw],
  );

  // The note appears by itself, and so does the reply being written.
  check("a teammate's note appears by itself", await waitFor(() => pageHas("Please look at this"), 15000));
  check("a mention counts as a note on the row", await waitFor(() => teamRow()?.innerText.includes("1 note"), 15000));
  check("I can see my teammate replying", await waitFor(() => document.querySelector(".team-replying")?.innerText.includes("sam@team.test is replying"), 20000));
  check("and on the list too", await waitFor(() => teamRow()?.innerText.includes("is replying"), 10000));

  await press(() => document.querySelector(".quick-reply"));
  await waitFor(() => document.querySelector(".composer-inline .ProseMirror"));
  await sleep(300);
  await typeInEditor("Boss got there first");
  await sleep(1000);
  const before = await outboxCount();
  await press(() => [...document.querySelectorAll(".composer-inline button")].find((b) => b.innerText.includes("Send")));
  check("my reply goes out", !!(await nextSent(before)));
  await shareSet("bossSent", "1");
  await shareWait("samDone", 60000);

  // Done, then back.
  await waitFor(() => teamRow());
  await press(() => teamRow());
  (await waitFor(() => document.querySelector(".team-assign"))).click();
  (await menuItem("Mark done")).click();
  check("done leaves the Open view", await waitFor(() => !teamRow(), 15000));
  await press(() => tabNamedT("Done"));
  check("done is in the Done view", await waitFor(() => teamRow(), 15000));
  await shareSet("markedDone", "1");
  await shareWait("samSawDone", 30000);

  // Out goes the teammate.
  await openSettings("Team");
  (await waitFor(() => document.querySelector('[data-team="Grayic"]'), 15000)).click();
  const samRow = await waitFor(() => document.querySelector('[data-member="sam@team.test"]'));
  await press(() => [...samRow.querySelectorAll("button")].find((b) => b.innerText === "Remove"));
  (await waitFor(() => buttonWith("Yes, remove"))).click();
  check("a teammate can be removed", await waitFor(() => !document.querySelector('[data-member="sam@team.test"]'), 15000));
  await closeSettings();
  await shareSet("removed", "1");
  await shareWait("samGone", 40000);

  // A second team, with the first left as it was.
  await openSettings("Team");
  (await waitFor(() => buttonWith("Start a team"))).click();
  setValue(await waitFor(() => labelled("Team name")), "Second");
  await sleep(50);
  await press(() => buttonWith("Create team"));
  check("a second team can be started", await waitFor(() => document.querySelector(".settings-page h1")?.innerText === "Second", 15000));
  await press(() => buttonWith("All teams"));
  check("both teams are listed", await waitFor(() => document.querySelector('[data-team="Grayic"]') && document.querySelector('[data-team="Second"]'), 15000));
  await press(() => document.querySelector('[data-team="Second"]'));
  await waitFor(() => document.querySelector(".settings-page h1")?.innerText === "Second");
  check("a new team starts with nothing shared", !document.querySelector("[data-inbox] .pill"));
  await closeSettings();
});
