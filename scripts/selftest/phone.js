// The real iPhone screens in the simulator, against the demo mailbox.
guarded(async () => {
  const shown = (el) => !!el && el.getBoundingClientRect().width > 0;
  const pane = () => document.querySelector(".app")?.dataset.pane;
  check("the phone starts by connecting an inbox", await waitFor(() => pageHas("Welcome to Mailzo"), 20000));
  check("it fits the phone's width", document.documentElement.scrollWidth <= window.innerWidth, document.documentElement.scrollWidth);
  await connectDemoInbox("demo-password");
  await waitFor(() => pageHas("Maya Chen"), 30000);
  await setUpAccount();
  const key = await waitFor(() => dialog("Recovery key"), 20000);
  check("the code was found and setup began", !!key);
  await press(() => document.querySelector('input[type="checkbox"]'));
  await sleep(50);
  await press(() => buttonWith("Continue"));
  check("mail arrives on the phone", await waitFor(() => pageHas("Maya Chen"), 30000));
  check("secrets are kept in the phone's Keychain", await (async () => {
    const accounts = await invoke("call", { cmd: "list_accounts", args: {} });
    return accounts.length === 1;
  })());

  check("the list fills the screen", await waitFor(() => shown(document.querySelector(".list")) && !shown(document.querySelector(".sidebar"))), JSON.stringify({ w: window.innerWidth, pane: pane(), text: document.body.innerText.slice(0, 200), list: document.querySelector(".list")?.getBoundingClientRect().width, side: document.querySelector(".sidebar")?.getBoundingClientRect().width }));
  check("nothing runs off the side", document.documentElement.scrollWidth <= window.innerWidth, document.documentElement.scrollWidth);

  // Folders
  await press(() => labelled("Folders"));
  check("the folders take over the screen", await waitFor(() => pane() === "nav" && shown(document.querySelector(".sidebar")) && !shown(document.querySelector(".list"))));
  await press(() => folderButton("All Inbox"));
  check("picking one goes back to the list", await waitFor(() => pane() === "list" && shown(document.querySelector(".list"))));

  // Reading
  await press(() => [...document.querySelectorAll(".thread")].find((r) => r.innerText.includes("Maya Chen")));
  check("opening an email fills the screen", await waitFor(() => pane() === "reader" && shown(document.querySelector("section h1")) && !shown(document.querySelector(".list"))));
  check("the email text is there", await waitFor(() => document.querySelector(".pane-reader")?.innerText.includes("Q4 launch plan")));
  await press(() => labelled("Back to the list"));
  check("back returns to the list", await waitFor(() => pane() === "list"));

  // Choosing several
  document.querySelector(".thread").dispatchEvent(new MouseEvent("click", { bubbles: true, metaKey: true }));
  check("choosing several stays on the list, with the actions below it", await waitFor(() => pane() === "list" && shown(document.querySelector(".list")) && shown(document.querySelector(".bulk"))));
  await press(() => buttonWith("Clear selection"));
  await waitFor(() => !shown(document.querySelector(".bulk")));

  // Writing
  await press(() => [...document.querySelectorAll(".thread")].find((r) => r.innerText.includes("Maya Chen")));
  await waitFor(() => pane() === "reader");
  const before = await outboxCount();
  await press(() => labelled("Back to the list"));
  await waitFor(() => pane() === "list");
  await invoke("call", { cmd: "set_setting", args: { key: "undo_send", value: "0" } });
  await press(() => labelled("Folders"));
  await waitFor(() => pane() === "nav");
  await press(() => buttonWith("New message"));
  const editor = await waitFor(() => document.querySelector(".ProseMirror"));
  check("a new message fills the screen", await waitFor(() => document.querySelector(".composer").getBoundingClientRect().width >= window.innerWidth - 1), document.querySelector(".composer").getBoundingClientRect().width);
  setValue(await waitFor(() => labelled("To")), "friend@inbox.test");
  pressEnter(labelled("To"));
  setValue(await waitFor(() => labelled("Subject")), "From the phone");
  await typeInEditor("Sent from the phone.");
  await sleep(300);
  await press(() => buttonWith("Send"));
  const sent = await nextSent(before);
  check("it really sends", sent && sent.data.includes("From the phone"), JSON.stringify(sent)?.slice(0, 200));
  await waitFor(() => !document.querySelector(".composer"));
  if (pane() === "nav") folderButton("All Inbox").click();

  // Calendar
  await press(() => labelled("Folders"));
  await waitFor(() => pane() === "nav");
  await press(() => [...document.querySelectorAll("nav .nav-item")].find((b) => b.innerText.includes("Calendar")));
  check("the calendar shows the month", await waitFor(() => document.querySelector(".calendar-grid") && shown(document.querySelector(".calendar-grid"))));
  check("and it fits", document.querySelector(".calendar-grid").getBoundingClientRect().width <= window.innerWidth);
  await press(() => labelled("Folders"));
  await waitFor(() => pane() === "nav");
  await press(() => folderButton("All Inbox"));
  await waitFor(() => pane() === "list");

  // Settings
  await invoke("call", { cmd: "app_settings", args: {} });
  await press(() => labelled("Folders"));
  await waitFor(() => pane() === "nav");
  await press(() => [...document.querySelectorAll(".sidebar-actions .nav-item")].find((b) => b.innerText.includes("Settings")));
  const nav = await waitFor(() => document.querySelector(".settings-nav"));
  check("settings start with the list of pages", shown(nav) && !shown(document.querySelector(".settings-page")));
  await press(() => [...document.querySelectorAll(".settings-nav .nav-item")].find((b) => b.innerText.includes("Privacy")));
  check("a page then fills the screen", await waitFor(() => shown(document.querySelector(".settings-page")) && !shown(nav)));
  await press(() => labelled("Back to settings"));
  check("and back returns to the list", await waitFor(() => shown(document.querySelector(".settings-nav"))));
  await invoke("selftest_share", { name: "phoneDone", value: "1" });
});
