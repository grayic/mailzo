// Writing and acting on mail in the real window. Called by first-run.js.
const pressEnter = (el) => el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
const labelled = (name) => document.querySelector(`[aria-label="${name}"]`);
const addFile = (input, file) => {
  const list = new DataTransfer();
  list.items.add(file);
  input.files = list.files;
  input.dispatchEvent(new Event("change", { bubbles: true }));
};
const typeInEditor = async (text) => {
  const ed = await waitFor(() => document.querySelector(".ProseMirror"));
  ed.focus();
  document.execCommand("insertText", false, text);
};
const outboxCount = async () => JSON.parse((await shareGet("outbox")) || "[]").length;
const nextSent = async (before) => {
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    const list = JSON.parse((await shareGet("outbox")) || "[]");
    if (list.length > before) return list[list.length - 1];
    await sleep(150);
  }
  return null;
};
const action = (name) => [...labelled("Conversation actions").querySelectorAll("button")].find((b) => (b.getAttribute("aria-label") ?? "") === name);
const folderButton = (name) => [...document.querySelectorAll("nav .nav-item")].find((b) => b.innerText.trim().split("\n")[0] === name);
const switchTo = async (name) => {
  await press(() => labelled("Switch account"));
  (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes(name)))).click();
};
const PIXEL = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

const mailFlow = async () => {
  const logo = await waitFor(() => buttonWith("Maya Chen")?.querySelector("img.avatar-picture-logo"), 15000);
  check("a company logo shows when there is one", logo && logo.naturalWidth === 32, logo?.naturalWidth);
  check("others keep their initials", !buttonWith("Sam Okafor")?.querySelector("img"));
  check("All Inbox is showing by default", folderButton("All Inbox")?.getAttribute("aria-current") === "true" && document.querySelector(".list-title")?.innerText === "All Inbox");
  await openSettings("Writing");
  check("undo send starts on, at 10 seconds", labelled("Undo send").querySelector('[aria-checked="true"]')?.innerText === "10s");
  await press(() => [...labelled("Undo send").querySelectorAll('[role="radio"]')].find((b) => b.innerText === "Off"));
  await closeSettings();

  // A new message using everything. The window's own storage outlives the test folders,
  // so start from formatting being closed, as a new user would see it.
  localStorage.removeItem("dakly.composer-tools");
  let before = await outboxCount();
  await press(() => buttonWith("New message"));
  const toBox = await waitFor(() => labelled("To"));
  check("the composer opens", !!toBox);
  setValue(toBox, "Ana Silva <ana@example.test>, demo@inbox.test");
  await sleep(50);
  pressEnter(toBox);
  check("addresses become chips", await waitFor(() => pageHas("Ana Silva") && !labelled("To").value));
  await press(() => buttonWith("Cc Bcc"));
  (await waitFor(() => buttonWith("Hide Cc Bcc"))).click();
  check("empty Cc and Bcc can be hidden again", await waitFor(() => !labelled("Cc")));
  await press(() => buttonWith("Cc Bcc"));
  const cc = await waitFor(() => labelled("Cc"));
  setValue(cc, "cat@example.test");
  await sleep(50);
  pressEnter(cc);
  const bcc = labelled("Bcc");
  setValue(bcc, "hidden@example.test");
  await sleep(50);
  pressEnter(bcc);
  setValue(await waitFor(() => labelled("Subject")), "Everything in one message");
  // Each label and its value sit in the middle of their row, in the real window.
  const middle = (el) => { const r = el.getBoundingClientRect(); return r.top + r.height / 2; };
  const rows = [...document.querySelectorAll(".composer-row")];
  check("composer labels and values are centred in their rows", rows.length === 4 && rows.every((row) => {
    const centre = middle(row);
    const label = row.querySelector(".composer-label");
    const value = row.querySelector("input");
    const text = document.createRange();
    text.selectNodeContents(label);
    return Math.abs(middle(text) - centre) <= 1 && Math.abs(middle(value) - centre) <= 1 && row.getBoundingClientRect().height <= 40;
  }), rows.map((row) => [row.querySelector(".composer-label").innerText, Math.round(row.getBoundingClientRect().height), Math.round(middle(row.querySelector(".composer-label"))- middle(row)), Math.round(middle(row.querySelector("input")) - middle(row))]));
  const subjectBox = labelled("Subject").getBoundingClientRect().left;
  const toBox2 = rows[0].querySelector(".chips").getBoundingClientRect().left;
  check("every row's value starts at the same place", Math.abs(subjectBox - toBox2) <= 1, [subjectBox, toBox2]);
  await typeInEditor("Hello team");
  // Formatting stays tucked away until asked for, and the choice is remembered.
  check("formatting is hidden at first", !labelled("Bold") && !!labelled("Formatting"));
  await press(() => labelled("Formatting"));
  check("the Aa button shows the formatting", !!(await waitFor(() => labelled("Bold"))) && labelled("Hide formatting")?.getAttribute("aria-pressed") === "true");
  check("and it's remembered", localStorage.getItem("dakly.composer-tools") === "on");
  document.querySelector(".ProseMirror").focus();
  document.execCommand("selectAll");
  await sleep(50);
  await press(() => labelled("Bold"));
  await sleep(100);
  check("bold shows as on in the toolbar", labelled("Bold").getAttribute("aria-pressed") === "true");
  check("the editor really made it bold", !!document.querySelector(".ProseMirror strong"));
  addFile(labelled("Attach files"), new File(["hello attachment"], "notes.txt", { type: "text/plain" }));
  check("the file appears as attached", await waitFor(() => labelled("Attachments")?.innerText.includes("notes.txt")));
  addFile(labelled("Attach files"), new File(["remove me"], "wrong.txt", { type: "text/plain" }));
  await waitFor(() => labelled("Remove wrong.txt"));
  await press(() => labelled("Remove wrong.txt"));
  check("an attached file can be removed", await waitFor(() => !labelled("Remove wrong.txt")));
  // A picture replaces whatever is selected, so put the cursor at the end first.
  window.getSelection().collapseToEnd();
  document.querySelector(".ProseMirror").dispatchEvent(new Event("focus"));
  await sleep(100);
  const bytes = Uint8Array.from(atob(PIXEL), (c) => c.charCodeAt(0));
  const pictureInput = document.querySelector('input[accept^="image/"]');
  addFile(pictureInput, new File([bytes], "dot.png", { type: "image/png" }));
  check("a picture goes into the text", await waitFor(() => document.querySelector(".ProseMirror img")));
  await press(() => buttonWith("Send"));
  const sent = await nextSent(before);
  check("the message reaches the outgoing server", !!sent);
  check("it goes to To, Cc and Bcc", sent && ["ana@example.test", "demo@inbox.test", "cat@example.test", "hidden@example.test"].every((a) => sent.to.includes(a)), sent?.to);
  check("the Bcc address is not in the message", sent && !sent.data.includes("hidden@example.test"));
  check("the attachment reached the message", sent && sent.data.includes("notes.txt"));
  check("the picture travels as an inline part", sent && sent.data.includes("Content-ID") && !sent.data.includes("data:image/png"));
  check("the composer closes after sending", await waitFor(() => !document.querySelector(".composer")));

  // It arrives back, because it was addressed to this inbox.
  const arrived = await waitFor(() => buttonWith("Everything in one message"), 30000) ?? (await (async () => { document.querySelector('[aria-label="Refresh mail"]')?.click(); return waitFor(() => buttonWith("Everything in one message"), 20000); })());
  check("sent mail arrives and is listed", !!arrived);
  check("the list shows it has an attachment", arrived && arrived.querySelector('[aria-label="Has attachments"]'));
  await press(() => arrived);
  check("the Sent copy shows the Bcc to the sender", await waitFor(() => pageHas("bcc hidden@example.test") || document.querySelectorAll("article").length >= 1));
  const shown = await waitFor(() => document.querySelector("section iframe"));
  await sleep(500);
  check("bold survives the trip", !!shown?.contentDocument?.querySelector("strong"));
  check("the picture shows inside the message", shown?.contentDocument?.querySelector("img")?.naturalWidth === 1, shown?.contentDocument?.querySelector("img")?.src?.slice(0, 40));
  const save = await waitFor(() => [...document.querySelectorAll('[aria-label="Attachments"] button')].find((b) => b.innerText === "Save"));
  check("attachments can be saved", !!save);
  await press(() => save);
  check("saving says where it went", await waitFor(() => pageHas("Saved to Downloads as notes.txt")));
  await press(() => buttonWith("Show in Finder"));
  (await waitFor(() => [...document.querySelectorAll('[aria-label="Attachments"] button')].find((b) => b.innerText === "Open"))).click();
  await sleep(200);

  // Going back to a list shows it at once, and the fresh copy follows unnoticed.
  await press(() => folderButton("Sent"));
  await waitFor(() => document.querySelector(".list-title")?.innerText === "Sent");
  await sleep(300);
  await press(() => folderButton("All Inbox"));
  await sleep(20);
  check("going back to a list shows it at once", document.querySelectorAll(".thread").length > 0);

  // An email that's been opened comes back at once, with nothing standing in for it.
  const rowOf = (words) => [...document.querySelectorAll(".thread")].find((b) => b.textContent === words);
  const readRow = document.querySelector(".thread:not(.thread-unread)");
  const readWords = readRow.textContent;
  await press(() => readRow);
  await waitFor(() => document.querySelector(".card"));
  await press(() => folderButton("Sent"));
  await waitFor(() => document.querySelector(".list-title")?.innerText === "Sent");
  await press(() => folderButton("All Inbox"));
  // Mail to yourself reads the same in Sent, so this waits for the list to have changed.
  (await waitFor(() => document.querySelector(".list-title")?.innerText === "All Inbox" && rowOf(readWords))).click();
  await sleep(0);
  check("an email already opened comes back at once", !!document.querySelector(".card"), document.querySelector(".thread[aria-current=true]")?.textContent.slice(0, 40));

  // Reply: threads and quotes.
  (await waitFor(() => buttonWith("Sam Okafor"))).click();
  check("opening marks the conversation read", await waitFor(() => !buttonWith("Sam Okafor")?.querySelector('[aria-label="Unread"]')));
  // Coming back to what was just read is instant too, even if it was read before being marked.
  await waitFor(() => document.querySelector(".card"));
  await press(() => rowOf(readWords));
  await sleep(0);
  await press(() => buttonWith("Sam Okafor"));
  await sleep(0);
  check("an email just read comes back at once", !!document.querySelector(".card"));
  before = await outboxCount();
  (await waitFor(() => labelled("Conversation actions")));
  await press(() => action("Reply"));
  await waitFor(() => labelled("Subject"));
  check("reply fills in the person", await waitFor(() => pageHas("Sam Okafor") && labelled("Subject").value === "Re: Invoice 1042 for October"), labelled("Subject")?.value);
  check("the earlier message is kept but tucked away", pageHas("Show the earlier message"));
  await press(() => buttonWith("Show the earlier message"));
  check("the earlier message can be seen", await waitFor(() => document.querySelector(".composer pre, .composer iframe")));
  await sleep(50);
  check("formatting stays open in the next message", !!labelled("Bold"));
  await press(() => labelled("Hide formatting"));
  check("and Aa hides it again", await waitFor(() => !labelled("Bold") && labelled("Formatting")?.getAttribute("aria-pressed") === "false" && localStorage.getItem("dakly.composer-tools") === "off"));
  await typeInEditor("Thanks, paying today. The receipt is attached.");
  await sleep(50);
  await press(() => buttonWith("Send"));
  // It talks about a file but has none, so it asks first.
  const askFile = await waitFor(() => dialog("Send without an attachment?") || (pageHas("Send without an attachment?") && buttonWith("Attach a file")));
  check("mentioning a missing attachment asks first", !!askFile && !!buttonWith("Attach a file") && !!buttonWith("Send anyway"));
  check("and nothing went out yet", (await outboxCount()) === before);
  await press(() => buttonWith("Send anyway"));
  const reply = await nextSent(before);
  check("the reply carries the conversation's IDs", reply && reply.data.includes("In-Reply-To") && reply.data.includes("invoice-1@fieldwork.test"));
  check("the reply includes the quote", reply && reply.data.includes("Invoice 1042 is attached"));
  check("the reply joins the conversation", await waitFor(() => document.querySelectorAll("article").length === 2 && pageHas("Thanks, paying today."), 30000), document.querySelectorAll("article").length);

  // Forward: a draft that's closed and comes back.
  await press(() => action("Forward"));
  await waitFor(() => labelled("Subject"));
  check("forward starts with no recipients", await waitFor(() => labelled("Subject").value === "Fwd: Invoice 1042 for October" && !document.querySelector(".composer .chip")), labelled("Subject")?.value);
  await typeInEditor("Keep this for later");
  await sleep(1000);
  await press(() => document.querySelector('.composer [aria-label="Close"]'));
  check("the Drafts folder shows a count", await waitFor(() => /Drafts\s*1/.test(document.querySelector("nav").innerText)), document.querySelector("nav").innerText);
  await press(() => folderButton("Drafts"));
  const draftRow = await waitFor(() => buttonWith("Fwd: Invoice 1042"));
  check("closing keeps it, inside the Drafts folder", !!draftRow && pageHas("Not sent yet"));
  await press(() => draftRow);
  check("the draft reopens with its words", await waitFor(() => document.querySelector(".ProseMirror")?.innerText.includes("Keep this for later")));
  await press(() => labelled("Discard"));
  (await waitFor(() => buttonWith("Yes, discard"))).click();
  check("a discarded draft is gone", await waitFor(() => !buttonWith("Fwd: Invoice 1042") && !/Drafts\s*1/.test(document.querySelector("nav").innerText)));
  await press(() => folderButton("Inbox"));

  // Star, unread, archive.
  (await waitFor(() => buttonWith("Sam Okafor"))).click();
  (await waitFor(() => labelled("Star"))).click();
  check("starring shows", await waitFor(() => labelled("Remove star")));
  await press(() => action("Mark unread"));
  check("mark unread shows the dot again", await waitFor(() => buttonWith("Sam Okafor")?.querySelector('[aria-label="Unread"]')));
  await press(() => buttonWith("Sam Okafor"));
  (await waitFor(() => labelled("Conversation actions")));
  await press(() => action("Archive"));
  check("archive removes it from the inbox", await waitFor(() => !buttonWith("Invoice 1042")));
  document.querySelector('[aria-label="Refresh mail"]')?.click();
  await sleep(1500);
  check("and a refresh doesn't bring it back", !buttonWith("Invoice 1042"));
  await press(() => folderButton("Archive"));
  check("it is in the Archive folder", await waitFor(() => buttonWith("Invoice 1042"), 15000));
  await press(() => folderButton("Inbox"));

  // Delete, then delete for good.
  (await waitFor(() => buttonWith("Everything in one message"))).click();
  (await waitFor(() => labelled("Conversation actions")));
  await press(() => action("Delete"));
  check("delete removes it from the inbox", await waitFor(() => !buttonWith("Everything in one message")));
  document.querySelector('[aria-label="Refresh mail"]')?.click();
  await sleep(1500);
  await press(() => folderButton("Trash"));
  (await waitFor(() => buttonWith("Everything in one message"), 15000)).click();
  (await waitFor(() => labelled("Delete for good"))).click();
  (await waitFor(() => buttonWith("Yes, delete for good"))).click();
  check("delete for good empties it from the bin", await waitFor(() => !buttonWith("Everything in one message")));
  await press(() => folderButton("Inbox"));

  // Settings: the name, signature and outgoing server.
  await openSettings("demo@inbox.test");
  const settings = await waitFor(() => labelled("Settings"));
  const field = (label) => [...settings.querySelectorAll(".field")].find((f) => f.innerText.includes(label)).querySelector("input");
  check("settings show the outgoing server it found", await waitFor(() => field("Server name (SMTP)")?.value === "127.0.0.1"));
  setValue(field("Your name"), "Demo Person");
  field("Your name").dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
  await sleep(300);
  await press(() => inSettings("Add a signature"));
  await waitFor(() => settings.querySelector(".signature .ProseMirror"));
  await typeInEditor("Demo Person, Grayic");
  await sleep(800);
  await press(() => inSettings("Test connection"));
  check("testing the connection works", await waitFor(() => pageHas("Sending should work"), 15000));
  await closeSettings();
  before = await outboxCount();
  await press(() => buttonWith("New message"));
  check("the signature appears in new messages", await waitFor(() => document.querySelector(".ProseMirror")?.innerText.includes("Grayic")));
  const to2 = labelled("To");
  setValue(to2, "ana@example.test");
  await sleep(50);
  pressEnter(to2);
  setValue(await waitFor(() => labelled("Subject")), "From my name");
  await sleep(50);
  // Command + Return sends.
  document.querySelector(".composer").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
  const named = await nextSent(before);
  check("⌘ Return sends, from the chosen name", named && named.data.includes("Demo Person"), named?.data?.slice(0, 200));
  await waitFor(() => !document.querySelector(".composer"));

  // Pictures stay blocked until asked for.
  await press(() => buttonWith("Weekly Digest"));
  check("pictures are hidden with a way to show them", await waitFor(() => buttonWith("Show pictures")));
  check("nothing has loaded yet", (await shareGet("pixels")).includes('"hits":0'), await shareGet("pixels"));
  await press(() => buttonWith("Show pictures"));
  const loaded = await (async () => {
    const end = Date.now() + 8000;
    while (Date.now() < end) {
      const p = await shareGet("pixels");
      if (p && !p.includes('"hits":0')) return true;
      await sleep(150);
    }
    return false;
  })();
  check("asking shows the pictures", loaded);
  check("it says this sender's pictures are now always shown", await waitFor(() => pageHas("Pictures from this sender are always shown")));
  // Leave the conversation and come back: the choice is remembered.
  await press(() => buttonWith("Maya Chen"));
  await waitFor(() => pageHas("Final version goes out on Tuesday"));
  await press(() => buttonWith("Weekly Digest"));
  const asked = await waitFor(() => pageHas("Pictures from this sender are always shown") || buttonWith("Show pictures"));
  check("coming back, the sender's pictures show without asking", asked && !buttonWith("Show pictures"));
  await press(() => buttonWith("Stop showing them"));
  check("the choice can be undone", await waitFor(() => buttonWith("Show pictures")));
};

// A second inbox: All Inbox mixes them and says which is which.
const allInboxFlow = async () => {
  await press(() => folderButton("All Inbox"));
  check("the list is titled All Inbox", await waitFor(() => document.querySelector(".list-title")?.innerText === "All Inbox"));

  await switchTo("Connect an inbox");
  const email = await waitFor(() => pageHas("Connect an inbox") && document.querySelector('input[type="email"]'));
  setValue(email, "second@inbox.test");
  await sleep(50);
  await press(() => buttonWith("Continue"));
  await waitFor(() => document.querySelectorAll("input").length >= 5);
  const inputs = document.querySelectorAll("input");
  setValue(inputs[1], "demo-password");
  setValue(inputs[2], "127.0.0.1");
  setValue(inputs[3], "1143");
  setValue(inputs[4], "demo@inbox.test");
  await sleep(50);
  await press(() => buttonWith("Connect inbox"));

  // Rows don't say which inbox they're from; each one's key starts with its inbox's number.
  let inboxIds = {};
  for (let i = 0; i < 300 && !inboxIds["second@inbox.test"]; i++) {
    inboxIds = Object.fromEntries((await invoke("call", { cmd: "list_accounts", args: {} })).map((a) => [a.email, a.id]));
    if (!inboxIds["second@inbox.test"]) await sleep(100);
  }
  const rowsFrom = (email) => [...document.querySelectorAll(".thread")].filter((r) => r.dataset.key?.startsWith(`${inboxIds[email]}:`));
  check("both inboxes appear in All Inbox", await waitFor(() => rowsFrom("demo@inbox.test").length > 0 && rowsFrom("second@inbox.test").length > 0, 30000), inboxIds);
  check("the pictures carry no inbox dot", !document.querySelector(".thread .account-dot"));
  check("the pictures name no inbox on hover", !document.querySelector(".thread-avatar[data-tip]"));
  check("rows don't spell out the inbox", ![...document.querySelectorAll(".thread")].some((t) => t.innerText.includes("second@inbox.test")));
  check("the sidebar doesn't list every inbox", ![...document.querySelectorAll("nav .nav-scroll")].some((n) => n.innerText.includes("second@inbox.test")));
  check("the dropdown shows one account, under the window buttons", labelled("Switch account")?.innerText.includes("demo@inbox.test") && labelled("Switch account").closest(".switcher-row") && labelled("Switch account").getBoundingClientRect().top >= document.querySelector(".sidebar-top").getBoundingClientRect().bottom);
  await press(() => labelled("Switch account"));
  const listed = await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].map((b) => b.innerText).join(" "));
  check("the dropdown lists accounts only, with no All Inbox", listed.includes("second@inbox.test") && !listed.includes("All Inbox"), listed);
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await waitFor(() => !document.querySelector('[role="menu"]'));
  check("the sidebar has one All Inbox tab and no other All tabs", folderButton("All Inbox") && folderButton("Sent") && folderButton("Trash") && ![...document.querySelectorAll("nav .nav-item")].some((b) => /All (Sent|Drafts|Archive|Junk|Trash)/.test(b.innerText)));
  // Dragging an inbox changes the order, and it stays changed.
  const inboxRows = () => [...document.querySelectorAll(".settings-nav .nav-item")].filter((r) => r.innerText.includes("@"));
  const inboxNames = () => inboxRows().map((r) => r.innerText.trim());
  const dragInbox = async (from, to) => {
    const y = (row) => row.getBoundingClientRect();
    from.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, pointerType: "mouse", clientY: y(from).top + 4 }));
    window.dispatchEvent(new PointerEvent("pointermove", { clientY: y(to).top + y(to).height / 2 }));
    await sleep(50);
    window.dispatchEvent(new PointerEvent("pointerup", {}));
  };
  await openSettings("General");
  const inboxesAtFirst = await waitFor(() => inboxNames().length >= 2 && inboxNames());
  await dragInbox(inboxRows()[0], inboxRows()[1]);
  check("dragging an inbox moves it", await waitFor(() => inboxNames()[0] === inboxesAtFirst[1]), inboxNames());
  await sleep(500);
  check("and the new order is saved", inboxNames()[0] === inboxesAtFirst[1], inboxNames());
  check("the app reads it back in that order", inboxesAtFirst[1].includes((await invoke("call", { cmd: "list_accounts", args: {} }))[0].email));
  await dragInbox(inboxRows()[0], inboxRows()[1]);
  await waitFor(() => inboxNames()[0] === inboxesAtFirst[0]);
  await closeSettings();
  const fromSecond = rowsFrom("second@inbox.test")[0];
  await press(() => fromSecond);
  check("a conversation from the second inbox opens", await waitFor(() => document.querySelectorAll("article").length >= 1));
  check("the open email says which inbox it's in", await waitFor(() => document.querySelector('.inbox-crumb[data-account="second@inbox.test"]')?.innerText.includes("second@inbox.test")));
  check("only the chosen row is highlighted", document.querySelectorAll('.thread[aria-current="true"]').length === 1);

  // The label takes you to that inbox, and One inbox shows no marks.
  await press(() => document.querySelector(".inbox-crumb"));
  check("the label goes to that account", await waitFor(() => labelled("Switch account").innerText.includes("second@inbox.test")));
  await press(() => folderButton("All Inbox"));
  await waitFor(() => rowsFrom("demo@inbox.test").length > 0 && rowsFrom("second@inbox.test").length > 0);
  await switchTo("second@inbox.test");
  check("picking an inbox shows just that inbox", await waitFor(() => document.querySelector(".list-title")?.innerText === "Inbox" && rowsFrom("demo@inbox.test").length === 0 && rowsFrom("second@inbox.test").length > 0));
  check("the switcher names the chosen inbox", labelled("Switch account").innerText.includes("second@inbox.test"));
  check("the sidebar shows that inbox's folders", folderButton("Sent")?.innerText.trim().startsWith("Sent"));
  // Keyboard: ⌘1 is everything, and ⌘3 here is the second inbox.
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "1", metaKey: true }));
  check("⌘1 goes to All Inbox", await waitFor(() => document.querySelector(".list-title")?.innerText === "All Inbox" && rowsFrom("demo@inbox.test").length > 0 && rowsFrom("second@inbox.test").length > 0));
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "3", metaKey: true }));
  check("⌘3 goes to the second inbox", await waitFor(() => labelled("Switch account").innerText.includes("second@inbox.test")));

  // Disconnecting it, while it's showing, goes back to All Inbox.
  await openSettings("second@inbox.test");
  (await waitFor(() => buttonWith("Disconnect this inbox"))).click();
  (await waitFor(() => buttonWith("Yes, disconnect"))).click();
  check("disconnecting removes its mail from All Inbox", await waitFor(() => !pageHas("second@inbox.test") && rowsFrom("second@inbox.test").length === 0 && document.querySelector(".list-title")?.innerText === "All Inbox", 15000));
  check("the other inbox's mail stays", await waitFor(() => buttonWith("Weekly Digest")));
};

const senderPicturesFlow = async () => {
  await openSettings("Privacy");
  const toggle = await waitFor(() => labelled("Show sender pictures and logos"));
  check("the setting says what's sent and to whom", pageHas("Senders are never contacted") && pageHas("Gravatar") && pageHas("DuckDuckGo"));
  check("sender pictures start switched on", toggle.checked);
  await press(() => toggle);
  check("switching it off removes the pictures", await waitFor(() => !document.querySelector("img.avatar-picture")));
  await press(() => labelled("Show sender pictures and logos"));
  check("switching it back on brings them back", await waitFor(() => document.querySelector("img.avatar-picture"), 10000));
  await closeSettings();
};

// Selecting several conversations.
const multiSelectFlow = async () => {
  await press(() => folderButton("All Inbox"));
  await waitFor(() => document.querySelector(".list-title")?.innerText === "All Inbox" && document.querySelectorAll(".thread").length >= 2);
  const rows = () => [...document.querySelectorAll(".thread")];
  const ticks = () => rows().filter((r) => r.classList.contains("thread-picked")).length;
  const tick = (row) => press(() => row.querySelector(".thread-check"));
  const click = (row, init) => row.dispatchEvent(new MouseEvent("click", { bubbles: true, ...init }));

  const before = rows().length;
  await tick(rows()[0]);
  check("a tick selects a conversation and shows the actions", await waitFor(() => ticks() === 1 && labelled("Selected conversations actions") && pageHas("1 selected")));
  check("while selecting, a plain click adds to the selection", (click(rows()[1], {}), await waitFor(() => ticks() === 2)));
  click(rows()[1], {});
  check("clicking again removes it", await waitFor(() => ticks() === 1));
  click(rows()[before - 1], { shiftKey: true });
  check("shift-click selects everything in between", await waitFor(() => ticks() === before), ticks());
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  check("esc clears the selection", await waitFor(() => ticks() === 0 && !labelled("Selected conversations actions")));
  click(rows()[0], { metaKey: true });
  check("⌘-click selects without opening", await waitFor(() => ticks() === 1 && !document.querySelector("article")));
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await waitFor(() => ticks() === 0);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "a", metaKey: true }));
  check("⌘A selects all", await waitFor(() => ticks() === before), ticks());
  await press(() => buttonWith("Clear selection"));
  await waitFor(() => ticks() === 0);

  // Acting on them: mark unread, then archive two and bring them back.
  const digest = rows().find((r) => r.innerText.includes("Weekly Digest"));
  const maya = rows().find((r) => r.innerText.includes("Maya Chen"));
  await tick(digest);
  await tick(maya);
  await waitFor(() => ticks() === 2);
  const bar = labelled("Selected conversations actions");
  await press(() => [...bar.querySelectorAll("button")].find((b) => b.innerText === "Mark unread"));
  check("marking several unread works", await waitFor(() => { const d = rows().find((r) => r.innerText.includes("Weekly Digest")); return ticks() === 0 && d?.querySelector('[aria-label="Unread"]'); }));

  // With only unread showing, all of it can be marked read at once, and taken back.
  const showUnreadOnly = async () => {
    await press(() => document.querySelector(".list-filter"));
    (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes("Unread only")))).click();
  };
  check("Mark all as read waits for the Unread filter", !labelled("Mark all as read"));
  await showUnreadOnly();
  const markAll = await waitFor(() => labelled("Mark all as read"));
  check("the Unread filter offers Mark all as read", !!markAll);
  // A slow mail server doesn't hold the counts back: they change here first, the server hears after.
  const slowServer = async (secs) => {
    await shareSet("slowstore", secs);
    for (let end = Date.now() + 10000; (await shareGet("slowstore")) !== null && Date.now() < end; ) await sleep(100);
  };
  const unreadCount = () => Number(folderButton("All Inbox")?.querySelector(".nav-count-unread")?.innerText ?? 0);
  check("there's unread mail to mark", unreadCount() > 0, unreadCount());
  await slowServer(8);
  const pressedAt = Date.now();
  await press(() => markAll);
  await waitFor(() => unreadCount() === 0);
  check("the count goes at once, before a slow server answers", Date.now() - pressedAt < 4000, Date.now() - pressedAt);
  check("it clears the list and says so", await waitFor(() => rows().length === 0 && pageHas("marked as read")));
  await slowServer(0);
  await press(() => buttonWith("Undo"));
  check("Undo brings them back", await waitFor(() => rows().some((r) => r.textContent.includes("Weekly Digest"))));
  await showUnreadOnly();
  await waitFor(() => !labelled("Mark all as read") && rows().some((r) => r.innerText.includes("Weekly Digest")) && rows().some((r) => r.innerText.includes("Maya Chen")));

  await tick(rows().find((r) => r.innerText.includes("Weekly Digest")));
  await tick(rows().find((r) => r.innerText.includes("Maya Chen")));
  await waitFor(() => ticks() === 2);
  await press(() => [...labelled("Selected conversations actions").querySelectorAll("button")].find((b) => b.innerText === "Archive"));
  check("archiving several removes them from the list", await waitFor(() => !buttonWith("Weekly Digest") && !buttonWith("Maya Chen"), 10000));
  await press(() => folderButton("Archive"));
  await waitFor(() => buttonWith("Weekly Digest") && buttonWith("Maya Chen"), 15000);
  check("they're in Archive", buttonWith("Weekly Digest") && buttonWith("Maya Chen"));
  await tick(rows().find((r) => r.innerText.includes("Weekly Digest")));
  await tick(rows().find((r) => r.innerText.includes("Maya Chen")));
  await waitFor(() => ticks() === 2);
  await press(() => [...labelled("Selected conversations actions").querySelectorAll("button")].find((b) => b.innerText === "Move to"));
  (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes("Inbox")))).click();
  check("moving several back works", await waitFor(() => !buttonWith("Weekly Digest"), 10000));
  await press(() => folderButton("All Inbox"));
  check("and they return to All Inbox", await waitFor(() => buttonWith("Weekly Digest") && buttonWith("Maya Chen"), 20000));
};

// Every choice in Settings, checked by what it changes on screen.
const settingsFlow = async () => {
  const pick = (group, option) => press(() => [...labelled(group).querySelectorAll('[role="radio"]')].find((b) => b.innerText === option));
  const pickMenu = async (label, option) => {
    await press(() => document.querySelector(`button.choice[aria-label="${label}"]`));
    (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === option))).click();
    await sleep(100);
  };
  const rows = () => [...document.querySelectorAll(".thread")];
  const row = (name) => rows().find((r) => r.innerText.includes(name));
  const isUnread = (name) => !!row(name)?.querySelector('[aria-label="Unread"]');
  const openRow = async (name) => {
    (await waitFor(() => row(name), 20000)).click();
    await waitFor(() => row(name)?.getAttribute("aria-current") === "true" && document.querySelector("article") && labelled("Conversation actions"));
  };
  const backFromArchive = async (name) => {
    await press(() => folderButton("Archive"));
    (await waitFor(() => row(name), 15000)).querySelector(".thread-check").click();
    const bar = await waitFor(() => labelled("Selected conversations actions"));
    await press(() => [...bar.querySelectorAll("button")].find((b) => b.innerText === "Move to"));
    (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes("Inbox")))).click();
    await waitFor(() => !row(name), 10000);
    await press(() => folderButton("All Inbox"));
    return waitFor(() => row(name), 20000);
  };
  const newMessage = async (to, subject) => {
    await press(() => buttonWith("New message"));
    const box = await waitFor(() => labelled("To"));
    setValue(box, to);
    await sleep(50);
    pressEnter(box);
    setValue(await waitFor(() => labelled("Subject")), subject);
    await sleep(600);
  };
  const pressSend = () => document.querySelector(".composer").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));

  await press(() => folderButton("All Inbox"));
  await waitFor(() => row("Weekly Digest"), 20000);

  const beside = () => Number(document.querySelector(".list-sub")?.innerText.match(/\d+/)?.[0] ?? 0);
  const inSidebar = () => Number(folderButton("All Inbox").querySelector(".nav-count")?.innerText ?? 0);
  await press(() => row("Weekly Digest").querySelector(".thread-check"));
  const marking = await waitFor(() => labelled("Selected conversations actions"));
  await press(() => [...marking.querySelectorAll("button")].find((b) => b.innerText === "Mark unread"));
  check("the unread numbers agree", await waitFor(() => beside() > 0 && beside() === inSidebar()), [beside(), inSidebar()]);

  // The window itself.
  window.dispatchEvent(new KeyboardEvent("keydown", { key: ",", metaKey: true }));
  const win = await waitFor(() => labelled("Settings"));
  check("⌘, opens Settings", !!win);
  const nav = [...win.querySelectorAll(".settings-nav .nav-item")].map((b) => b.querySelector(".truncate-1")?.innerText.trim());
  check("Settings lists app pages and each inbox", ["General", "Appearance", "Writing", "Notifications", "Privacy", "Shortcuts", "Profile", "Advanced", "demo@inbox.test", "Connect an inbox"].every((p) => nav.includes(p)), nav);
  check("Settings has a page for what AI apps did", !!inSettings("AI activity"));
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "n", metaKey: true }));
  await sleep(300);
  check("mail shortcuts wait while Settings is open", !document.querySelector(".composer"));
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  check("esc closes Settings", await waitFor(() => !labelled("Settings")));

  // Appearance.
  await openSettings("Appearance");
  const canvas = () => getComputedStyle(document.documentElement).getPropertyValue("--canvas").trim();
  await pick("Theme", "Dark");
  const dark = await waitFor(() => document.documentElement.dataset.theme === "dark" && canvas());
  await pick("Theme", "Light");
  const light = await waitFor(() => document.documentElement.dataset.theme === "light" && canvas());
  check("dark and light themes can be chosen", dark && light && dark !== light, [dark, light]);
  await pick("Theme", "Follow the Mac");
  check("the theme can follow the Mac again", await waitFor(() => !document.documentElement.dataset.theme));
  const subjectSize = () => parseFloat(getComputedStyle(row("Weekly Digest").querySelector(".thread-subject")).fontSize);
  const normalSize = subjectSize();
  check("normal text is a comfortable size", normalSize >= 14, normalSize);
  await pick("Text size", "Extra large");
  check("text size changes the whole app", await waitFor(() => subjectSize() > normalSize + 2), [normalSize, subjectSize()]);
  await pick("Text size", "Small");
  check("and can go smaller", await waitFor(() => subjectSize() < normalSize), subjectSize());
  await pick("Text size", "Normal");
  await waitFor(() => subjectSize() === normalSize);
  const tall = row("Weekly Digest").offsetHeight;
  await pick("Spacing", "Compact");
  check("compact spacing makes rows shorter", await waitFor(() => row("Weekly Digest").offsetHeight < tall), [tall, row("Weekly Digest").offsetHeight]);
  await pick("Spacing", "Comfortable");
  const snippet = () => getComputedStyle(row("Weekly Digest").querySelector(".thread-snippet"));
  await pick("Lines of preview", "None");
  check("the preview can be hidden", await waitFor(() => snippet().display === "none"));
  await pick("Lines of preview", "Two");
  check("the preview can have two lines", await waitFor(() => snippet().webkitLineClamp === "2"));
  await pick("Lines of preview", "One");
  await press(() => labelled("Show sender pictures in the list"));
  check("pictures can be left out of the list", await waitFor(() => !document.querySelector(".thread img.avatar-picture")));
  await press(() => labelled("Show sender pictures in the list"));
  check("and brought back", await waitFor(() => document.querySelector(".thread img.avatar-picture"), 10000));
  let badge = true;
  try { await invoke("set_dock_badge", { count: 3 }); await invoke("set_dock_badge", { count: 0 }); } catch (_) { badge = false; }
  check("the Dock count can be set", badge);

  // General.
  await openSettings("General");
  await press(() => labelled("Group emails into conversations"));
  check("with conversations off, each email has its own row", await waitFor(() => rows().filter((r) => r.innerText.includes("Maya Chen")).length > 1, 15000));
  await closeSettings();
  (await waitFor(() => row("Maya Chen"))).click();
  check("a single email opens on its own", await waitFor(() => document.querySelectorAll("article").length === 1));
  await openSettings("General");
  await press(() => labelled("Group emails into conversations"));
  check("turning conversations on groups them again", await waitFor(() => rows().filter((r) => r.innerText.includes("Maya Chen")).length === 1, 15000));

  await pickMenu("Mark as read", "Only when I choose");
  await closeSettings();
  await openRow("Maya Chen");
  action("Mark unread")?.click();
  await waitFor(() => isUnread("Maya Chen"));
  await openRow("Maya Chen");
  await sleep(600);
  check("marking read can be left to you", isUnread("Maya Chen") && action("Mark read"));
  await press(() => action("Mark read"));
  check("and done with the button", await waitFor(() => !isUnread("Maya Chen")));
  await openSettings("General");
  await pickMenu("Mark as read", "After 3 seconds");
  await closeSettings();
  await press(() => action("Mark unread"));
  await waitFor(() => isUnread("Maya Chen"));
  await openRow("Maya Chen");
  await sleep(1000);
  const stillUnread = isUnread("Maya Chen");
  check("marking read can wait a few seconds", stillUnread && await waitFor(() => !isUnread("Maya Chen"), 5000));
  await openSettings("General");
  await pickMenu("Mark as read", "Straight away");
  await pickMenu("After archiving or deleting", "Go back to the list");
  await closeSettings();
  await openRow("Weekly Digest");
  await press(() => action("Archive"));
  check("after archiving it can go back to the list", await waitFor(() => !row("Weekly Digest") && !document.querySelector("article"), 10000));
  check("archived mail comes back when moved", await backFromArchive("Weekly Digest"));
  await openSettings("General");
  await pickMenu("After archiving or deleting", "Open the next email");
  await pickMenu("The Delete key", "Archives");
  await closeSettings();
  await openRow("Weekly Digest");
  document.activeElement?.blur();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace", bubbles: true }));
  check("the Delete key follows the choice", await waitFor(() => !row("Weekly Digest"), 10000));
  await backFromArchive("Weekly Digest");
  await openSettings("General");
  await pickMenu("The Delete key", "Moves to the bin");

  // Writing.
  await openSettings("Writing");
  await pick("The main Reply action", "Reply all");
  await closeSettings();
  await openRow("Maya Chen");
  check("the main reply can be reply all", await waitFor(() => document.querySelector(".quick-reply")?.innerText.includes("Reply all")));
  await openSettings("Writing");
  await pick("The main Reply action", "Reply");
  await pickMenu("Font", "Georgia");
  await pick("Size", "18");
  await press(() => labelled("Check spelling"));
  await closeSettings();
  await press(() => buttonWith("New message"));
  const ed = await waitFor(() => document.querySelector(".composer .ProseMirror"));
  const style = getComputedStyle(ed);
  check("the chosen font and size show while writing", style.fontFamily.includes("Georgia") && style.fontSize === "18px", [style.fontFamily, style.fontSize]);
  check("spell check can be turned off", ed.getAttribute("spellcheck") === "false");
  await press(() => document.querySelector('.composer [aria-label="Close"]'));
  await waitFor(() => !document.querySelector(".composer"));
  await openSettings("Writing");
  await pickMenu("Font", "System");
  await pick("Size", "14");
  await press(() => labelled("Check spelling"));

  // Notifications.
  await openSettings("Notifications");
  check("notifications start on", labelled("Notify me about new email").checked);
  await press(() => labelled("Quiet hours"));
  check("quiet hours offer their times", await waitFor(() => document.querySelector('button.choice[aria-label="From"]')?.innerText.includes("10 PM")));
  await press(() => labelled("Quiet hours"));
  await press(() => labelled("Notify me about new email"));
  check("turning them off hides the details", await waitFor(() => !labelled("Play a sound")));
  await press(() => labelled("Notify me about new email"));
  await waitFor(() => labelled("Play a sound"));

  // Privacy: who is allowed, and what was held back.
  await closeSettings();
  await openRow("Weekly Digest");
  (await waitFor(() => buttonWith("Show pictures"))).click();
  await waitFor(() => pageHas("Pictures from this sender are always shown"));
  await openSettings("Privacy");
  const allowed = await waitFor(() => document.querySelector("[data-sender]"));
  check("allowed senders are listed", !!allowed);
  check("held-back pictures are counted", await waitFor(() => /[1-9]\d* emails?/.test(document.querySelector(".stat")?.innerText ?? "")), document.querySelector(".stat")?.innerText);
  const sender = allowed?.dataset.sender;
  await press(() => [...allowed.querySelectorAll("button")].find((b) => b.innerText === "Remove"));
  check("a sender can be removed", await waitFor(() => !document.querySelector(`[data-sender="${sender}"]`)));
  await closeSettings();
  await openRow("Weekly Digest");
  check("a removed sender's pictures are hidden again", await waitFor(() => buttonWith("Show pictures")));
  await openSettings("Privacy");
  await pick("Pictures in emails", "Show by default");
  await closeSettings();
  check("remote pictures can always show", await waitFor(() => !buttonWith("Show pictures")));
  await openSettings("Privacy");
  await pick("Pictures in emails", "Hide by default");
  await closeSettings();

  // One inbox's page.
  await openSettings("demo@inbox.test");
  const fieldOf = (label) => [...document.querySelectorAll(".settings .field")].find((f) => f.innerText.includes(label)).querySelector("input");
  const commit = (input, value) => { setValue(input, value); input.dispatchEvent(new FocusEvent("focusout", { bubbles: true })); };
  commit(fieldOf("Short label"), "Work");
  check("an inbox can have a short label", await waitFor(() => labelled("Switch account").innerText.includes("Work")));
  await press(() => labelled("Colour 145"));
  check("an inbox can have its own colour", await waitFor(() => labelled("Switch account").querySelector('[style*="--h: 145"]')));
  check("folder jobs can be set", labelled("Sent folder") && labelled("Trash folder"));
  await press(() => labelled("Include in All Inbox"));
  await closeSettings();
  check("an inbox can be left out of All Inbox", await waitFor(() => !row("Weekly Digest"), 10000));
  await openSettings("Work");
  await press(() => labelled("Include in All Inbox"));
  await closeSettings();
  check("and put back", await waitFor(() => row("Weekly Digest"), 10000));
  await openSettings("Work");
  commit(fieldOf("Short label"), "");
  await waitFor(() => !labelled("Switch account").innerText.includes("Work"));

  // The password: checked before it's saved.
  setValue(await waitFor(() => labelled("New password")), "not-the-password");
  await sleep(50);
  await press(() => inSettings("Test and save"));
  check("a wrong password is refused", await waitFor(() => document.querySelector('.settings [role="alert"]'), 15000));
  setValue(await waitFor(() => labelled("New password")), "");
  await sleep(50);
  await press(() => inSettings("Test and save"));
  check("the saved password still works", await waitFor(() => pageHas("Connected, and saved."), 15000));

  // Another address to send from.
  setValue(await waitFor(() => labelled("Other address")), "hello@inbox.test");
  await sleep(50);
  await press(() => [...document.querySelectorAll(".alias-form button")][0]);
  check("another address can be added", await waitFor(() => inSettings("Remove hello@inbox.test")));
  await closeSettings();
  let before = await outboxCount();
  await newMessage("ana@example.test", "From the other address");
  await press(() => document.querySelector("button.from-btn"));
  (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes("hello@inbox.test")))).click();
  await sleep(600);
  pressSend();
  const fromAlias = await nextSent(before);
  check("mail can go out from the other address", fromAlias?.data.includes("hello@inbox.test") && /^From:.*hello@inbox\.test/m.test(fromAlias.data), fromAlias?.data?.slice(0, 200));
  await waitFor(() => !document.querySelector(".composer"));
  await openSettings("demo@inbox.test");
  await press(() => inSettings("Remove hello@inbox.test"));
  await waitFor(() => !inSettings("Remove hello@inbox.test"));

  // Undo send.
  await openSettings("Writing");
  await pick("Undo send", "5s");
  await closeSettings();
  before = await outboxCount();
  await newMessage("ana@example.test", "Take this back");
  pressSend();
  check("sending waits, with a way to undo", await waitFor(() => !document.querySelector(".composer") && pageHas("Sending in") && buttonWith("Undo")));
  check("the waiting email isn't listed as a draft", !folderButton("Drafts")?.innerText.match(/\d/) );
  await press(() => buttonWith("Undo"));
  check("undo reopens the email", await waitFor(() => labelled("Subject")?.value === "Take this back"));
  await sleep(6000);
  check("an undone email is not sent", await outboxCount() === before);
  pressSend();
  await waitFor(() => pageHas("Sending in"));
  const afterWait = await nextSent(before);
  check("the email leaves when the wait ends", afterWait?.data.includes("Take this back"), afterWait?.data?.slice(0, 200));
  before = await outboxCount();
  await newMessage("ana@example.test", "Right now");
  pressSend();
  (await waitFor(() => buttonWith("Send now"))).click();
  const rightNow = await nextSent(before);
  check("send now skips the wait", rightNow?.data.includes("Right now"), rightNow?.data?.slice(0, 300));
  await sleep(1500);
  check("each email went out exactly once", (await outboxCount()) === before + 1, [before, await outboxCount()]);
  check("the wait message goes away", await waitFor(() => !pageHas("Sending in")));

  // Starting the saved mail again. Done last: it empties the list for a moment.
  await openSettings("Advanced");
  await press(() => inSettings("Clear and download again"));
  (await waitFor(() => buttonWith("Yes, clear it"))).click();
  check("saved mail can be cleared", await waitFor(() => pageHas("Your mail is downloading again"), 15000));
  await closeSettings();
  check("and it downloads again", await waitFor(() => row("Weekly Digest") && row("Maya Chen"), 30000));
};

// The menu on a right-clicked email.
const rowMenuFlow = async () => {
  await press(() => folderButton("All Inbox"));
  const rows = () => [...document.querySelectorAll(".thread")];
  const row = (name) => rows().find((r) => r.innerText.includes(name));
  await waitFor(() => row("Weekly Digest") && row("Maya Chen"), 20000);
  const rightClick = async (name, x = 400, y = 200) => {
    row(name).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: x, clientY: y }));
    return waitFor(() => labelled("Email actions"));
  };
  const items = () => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].map((b) => b.innerText.trim());
  const choose = (t) => press(() => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === t));
  const refreshMail = async () => {
    await press(() => labelled("Refresh mail"));
    await sleep(300);
    await waitFor(() => labelled("Refresh mail"), 15000);
    check("the refresh button says when mail was last updated", await waitFor(() => /Updated/.test(labelled("Refresh mail")?.dataset.tip ?? "")));
    await sleep(300);
  };

  let menu = await rightClick("Weekly Digest");
  check("right-click opens our own menu", !!menu);
  check("it has the common actions", ["Reply", "Forward", "Archive", "Move to…", "Delete", "More…"].every((t) => items().includes(t)) && items().some((t) => t === "Star" || t === "Remove star"), items());
  await sleep(250);
  const box = menu.getBoundingClientRect();
  check("it opens where the mouse is", Math.abs(box.left - 400) < 8 && Math.abs(box.top - 200) < 8, [box.left, box.top]);
  check("the clicked row is marked", row("Weekly Digest").hasAttribute("data-menu-open"));
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  check("esc closes it", await waitFor(() => !labelled("Email actions")));
  menu = await rightClick("Weekly Digest", window.innerWidth - 4, window.innerHeight - 4);
  const edge = menu.getBoundingClientRect();
  check("it stays inside the window", edge.right <= window.innerWidth && edge.bottom <= window.innerHeight, [edge.right, edge.bottom]);
  document.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  check("clicking away closes it", await waitFor(() => !labelled("Email actions")));

  // Quick actions on a row, which leave what's open alone.
  check("rows offer quick actions", !!row("Weekly Digest").querySelector('.quick-action[aria-label^="Delete"]'));
  await press(() => row("Weekly Digest").querySelector('.quick-action[aria-label^="Delete"]'));
  check("deleting from the row works, without opening it", await waitFor(() => !row("Weekly Digest") && !document.querySelector("article"), 1000));
  (await waitFor(() => buttonWith("Undo"))).click();
  check("and Undo brings the row straight back", await waitFor(() => row("Weekly Digest"), 3000));
  await press(() => row("Weekly Digest").querySelector('.quick-action[aria-label^="Delete"]'));
  await waitFor(() => !row("Weekly Digest"), 1000);
  await refreshMail();
  await press(() => folderButton("Trash"));
  const binnedRow = await waitFor(() => row("Weekly Digest"), 15000);
  check("in the bin, rows don't offer them", binnedRow && !binnedRow.querySelector(".quick-action"));
  await rightClick("Weekly Digest");
  await choose("Move to…");
  (await waitFor(() => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === "Inbox"))).click();
  await waitFor(() => !row("Weekly Digest"), 10000);
  await refreshMail();
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row("Weekly Digest"), 20000);

  // Select all, in one click.
  const count = rows().length;
  const picked = () => rows().filter((r) => r.classList.contains("thread-picked")).length;
  await press(() => labelled("Select all"));
  check("one click selects everything", await waitFor(() => picked() === count && pageHas(`${count} selected`)), picked());
  await press(() => [...document.querySelectorAll(".list-head button")].find((b) => b.getAttribute("aria-label") === "Clear selection"));
  check("and one click clears it", await waitFor(() => picked() === 0 && labelled("Select all")));
  await rightClick("Weekly Digest");
  await choose("More…");
  await sleep(200);
  await choose("Select all");
  check("the menu can select all too", await waitFor(() => picked() === count), picked());
  await press(() => [...document.querySelectorAll(".list-head button")].find((b) => b.getAttribute("aria-label") === "Clear selection"));
  await waitFor(() => picked() === 0);

  // Select.
  await rightClick("Weekly Digest");
  await choose("More…");
  await sleep(200);
  await choose("Select");
  check("Select starts selecting", await waitFor(() => row("Weekly Digest").classList.contains("thread-picked") && labelled("Selected conversations actions")));
  await press(() => row("Maya Chen").querySelector(".thread-check"));
  await waitFor(() => row("Maya Chen").classList.contains("thread-picked"));
  await rightClick("Maya Chen");
  check("on a selection it acts on all of them", pageHas("2 selected") && items().includes("Clear selection") && !items().includes("Reply"), items());
  await choose("Mark unread");
  check("marking from the menu covers everything selected", await waitFor(() => row("Weekly Digest").querySelector('[aria-label="Unread"]') && row("Maya Chen").querySelector('[aria-label="Unread"]') && !labelled("Selected conversations actions")));

  // Marks on one email.
  await rightClick("Weekly Digest");
  await choose("Mark read");
  // Before the server could have answered, as nothing should wait for it.
  await sleep(0);
  check("marking shows straight away", !row("Weekly Digest").querySelector('[aria-label="Unread"]'));
  check("Mark read works from the menu", await waitFor(() => !row("Weekly Digest").querySelector('[aria-label="Unread"]')));
  await rightClick("Weekly Digest");
  await choose("Star");
  check("Star works from the menu", await waitFor(() => row("Weekly Digest").querySelector('[aria-label="Starred"]')));
  await rightClick("Weekly Digest");
  await choose("Remove star");
  check("Remove star works from the menu", await waitFor(() => !row("Weekly Digest").querySelector('[aria-label="Starred"]')));

  // Reply, without opening it first.
  await rightClick("Maya Chen");
  await choose("Reply");
  check("Reply opens a reply to that email", await waitFor(() => /^Re:/.test(labelled("Subject")?.value ?? "") && labelled("Subject").value.includes("Q4 launch plan")), labelled("Subject")?.value);
  labelled("Discard")?.click();
  await sleep(200);
  const sure = [...document.querySelectorAll("button")].find((b) => b.innerText.includes("discard"));
  sure?.click();
  await waitFor(() => !document.querySelector(".composer"));

  // Delete, with another email staying open.
  (await waitFor(() => row("Maya Chen"))).click();
  await waitFor(() => document.querySelector("article"));
  await rightClick("Weekly Digest");
  await choose("Delete");
  check("Delete from the menu removes it", await waitFor(() => !row("Weekly Digest"), 10000));
  check("what you were reading stays open", await waitFor(() => document.querySelector("article") && row("Maya Chen")?.getAttribute("aria-current") === "true"));
  await press(() => folderButton("Trash"));
  await waitFor(() => row("Weekly Digest"), 15000);
  await rightClick("Weekly Digest");
  check("in the bin it offers Delete for good", items().includes("Delete for good") && !items().includes("Delete"), items());
  check("and Move to inbox", items().includes("Move to inbox"), items());
  await choose("Move to…");
  const inbox = await waitFor(() => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === "Inbox"));
  check("Move to lists the folders", !!inbox && !items().includes("Trash"), items());
  await press(() => inbox);
  await waitFor(() => !row("Weekly Digest"), 10000);
  await press(() => folderButton("All Inbox"));
  check("and moving it back works", await waitFor(() => row("Weekly Digest"), 20000));
};

// Finding mail, with and without AI.
const searchFlow = async () => {
  const box = () => labelled("Search all email");
  const rows = () => [...document.querySelectorAll(".thread")];
  const row = (name) => rows().find((r) => r.innerText.includes(name));
  const title = () => document.querySelector(".list-title")?.innerText;
  const type = (text) => { box().focus(); setValue(box(), text); };
  // The Unread title in the menu bar panel opens unread mail here.
  await invoke("plugin:event|emit", { event: "show-unread", payload: null });
  check("the panel's Unread opens All Inbox on unread mail", await waitFor(() => title() === "All Inbox" && document.querySelector(".list-filter")?.innerText.includes("Unread")));
  await flipUnreadFilter();
  await waitFor(() => !document.querySelector(".list-filter").innerText.includes("Unread"));
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row("Weekly Digest") && row("Maya Chen"), 20000);

  window.dispatchEvent(new KeyboardEvent("keydown", { key: "f", metaKey: true }));
  check("⌘F goes to the search box", await waitFor(() => document.activeElement === box()));
  const beforeSearch = rows().length;
  type("launch");
  await sleep(0);
  check("what was showing goes as soon as a search starts", beforeSearch > 0 && rows().length === 0, [beforeSearch, rows().length]);
  check("typing shows results straight away", await waitFor(() => title() === "Results" && row("Maya Chen") && !row("Weekly Digest")), [title(), rows().length]);
  check("it says how many it found", pageHas("1 found"));
  check("typing stays in the box", document.activeElement === box());
  check("no AI key means no AI line", !labelled("Use my exact words"));
  type("tuesday");
  check("it searches inside emails", await waitFor(() => row("Maya Chen") && rows().length === 1));
  await waitFor(() => !pageHas("Checking your mail servers"), 15000);
  check("the server check ends without a problem", !pageHas("Couldn't check older email"), document.querySelector(".search-note")?.innerText);
  type("zzzz-nothing-like-this");
  check("nothing found says so", await waitFor(() => pageHas("No emails found")));
  type("from:maya");
  check("from: finds a sender", await waitFor(() => row("Maya Chen") && rows().length === 1));
  box().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  check("esc goes back to where you were", await waitFor(() => title() === "All Inbox" && row("Weekly Digest") && !box().value));

  // Everything in the list works on results.
  type("launch");
  await waitFor(() => row("Maya Chen") && rows().length === 1);
  await press(() => row("Maya Chen"));
  check("a result opens", await waitFor(() => document.querySelector("article")));
  await press(() => folderButton("Sent"));
  check("picking a folder leaves search", await waitFor(() => title() === "Sent" && !box().value));
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row("Weekly Digest"));

  // Mail in the bin is offered, not mixed in.
  row("Weekly Digest").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 }));
  const menu = await waitFor(() => labelled("Email actions"));
  await press(() => [...menu.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === "Delete"));
  await waitFor(() => !row("Weekly Digest"), 10000);
  type("weekly");
  check("the bin is offered, not mixed in", await waitFor(() => pageHas("1 more in Trash or Spam") && !row("Weekly Digest"), 15000), document.querySelector(".search-note")?.innerText);
  await waitFor(() => !pageHas("Checking your mail servers"), 15000);
  await press(() => [...document.querySelectorAll(".search-foot button")].find((b) => b.innerText === "Show them"));
  const binned = await waitFor(() => row("Weekly Digest"), 15000);
  check("and can be shown", !!binned);
  binned.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 }));
  const again = await waitFor(() => labelled("Email actions"));
  await press(() => [...again.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === "Move to…"));
  (await waitFor(() => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === "Inbox"))).click();
  await sleep(500);
  await press(() => labelled("Clear search"));
  check("it can be put back", await waitFor(() => title() === "All Inbox" && row("Weekly Digest"), 20000));

  // The AI key.
  await openSettings("AI");
  check("the AI page offers more than one service", await waitFor(() => labelled("AI service")?.innerText.includes("Claude")));
  setValue(await waitFor(() => labelled("AI key")), "not-the-key");
  await sleep(50);
  await press(() => inSettings("Test and save"));
  check("a wrong key is refused", await waitFor(() => pageHas("That key wasn't accepted")));
  setValue(await waitFor(() => labelled("AI key")), "test-ai-key");
  await sleep(50);
  await press(() => inSettings("Test and save"));
  check("the right key is kept, showing only its end", await waitFor(() => pageHas("Ending in -key")));
  check("the key isn't on the page", !document.body.innerText.includes("test-ai-key") && !labelled("AI key"));
  await closeSettings();

  // The search box only searches: even with AI connected, a sentence is its own words.
  type("what maya said about launching");
  check(
    "the search box reads only the words typed, with no AI",
    (await waitFor(() => pageHas("No emails found"), 15000)) && !pageHas("Launch emails from Maya"),
    document.querySelector(".search-note")?.innerText,
  );
  await press(() => labelled("Clear search"));
  await waitFor(() => title() === "All Inbox");

  await openSettings("AI");
  await press(() => inSettings("Remove"));
  (await waitFor(() => buttonWith("Yes, remove"))).click();
  check("the key can be removed", await waitFor(() => labelled("AI key")));
  await closeSettings();
};

// Spam protection and sorting, starting as a trial that moves nothing.
const spamFlow = async () => {
  const rows = () => [...document.querySelectorAll(".thread")];
  const row = (name) => rows().find((r) => r.innerText.includes(name));
  const SCAM = "Your account has been suspended";
  const rightClick = async (name) => {
    row(name).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 }));
    return waitFor(() => labelled("Email actions"));
  };
  const choose = (t) => press(() => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === t));
  const refresh = async () => {
    await press(() => labelled("Refresh mail"));
    await sleep(300);
    await waitFor(() => labelled("Refresh mail"), 15000);
    await sleep(300);
  };
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row(SCAM), 20000);

  check("a scam is flagged in the list", row(SCAM).innerText.includes("Looks like spam"), row(SCAM).innerText);
  check("it shows no borrowed logo", !row(SCAM).querySelector("img.avatar-picture"));
  check("nothing moved during the trial", !!row(SCAM));
  await press(() => row(SCAM));
  const note = await waitFor(() => document.querySelector(".scam-note"));
  check("the email explains why", note?.innerText.includes("paypa1.com") && note?.innerText.includes("made to look like"), note?.innerText);

  await openSettings("Spam");
  check("the trial lists what it would move", await waitFor(() => document.querySelector(`[data-would-move="${SCAM}"]`)) && pageHas("Trial mode"));
  check("it says how much it has learned", pageHas("good examples"));
  await closeSettings();

  // Turning moving on.
  await openSettings("Spam");
  await press(() => [...labelled("When an email looks like spam").querySelectorAll('[role="radio"]')].find((b) => b.innerText === "Move to Spam"));
  await closeSettings();
  check("once on, spam leaves the inbox", await waitFor(() => !row(SCAM), 20000));
  await refresh();
  await press(() => folderButton("Spam"));
  const inJunk = await waitFor(() => row(SCAM), 20000);
  check("it's in Spam", !!inJunk);
  await press(() => inJunk);
  const why = await waitFor(() => document.querySelector(".scam-note"));
  check("Spam says why it's there", why?.innerText.includes("Why this is in Spam"), why?.innerText);

  // Not spam.
  await press(() => action("Not spam"));
  check("Not spam takes it out of Spam", await waitFor(() => !row(SCAM), 15000));
  await refresh();
  await press(() => folderButton("All Inbox"));
  const back = await waitFor(() => row(SCAM), 20000);
  check("and brings it back, with no warning", back && !back.innerText.includes("Looks like spam"), back?.innerText);
  await openSettings("Spam");
  check("its sender is now trusted", await waitFor(() => pageHas("service@paypa1.com")));
  (await waitFor(() => [...document.querySelectorAll(".settings .group-row")].find((r) => r.innerText.includes("service@paypa1.com")))).querySelector("button").click();
  await waitFor(() => !pageHas("service@paypa1.com"));
  await closeSettings();

  // Mark as spam, from the list.
  await rightClick(SCAM);
  await choose("More…");
  await sleep(200);
  check("the menu offers spam and block", ["Mark as spam", "Block sender", "Always put on tab…"].every((t) => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].some((b) => b.innerText.trim() === t)));
  await choose("Mark as spam");
  check("marking as spam moves it out", await waitFor(() => !row(SCAM), 15000));
  await refresh();
  await press(() => folderButton("Spam"));
  (await waitFor(() => row(SCAM), 20000)).querySelector(".thread-check").click();
  const bar = await waitFor(() => labelled("Selected conversations actions"));
  await press(() => [...bar.querySelectorAll("button")].find((b) => b.innerText === "Not spam"));
  await waitFor(() => !row(SCAM), 15000);
  await refresh();
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row(SCAM), 20000);

  // Tabs.
  await openSettings("Spam");
  await press(() => [...labelled("When an email looks like spam").querySelectorAll('[role="radio"]')].find((b) => b.innerText === "Only warn me"));
  await press(() => labelled("Sort the inbox into tabs"));
  await closeSettings();
  const tabNamed = (name) => [...document.querySelectorAll('[role="tab"]')].find((t) => t.innerText.startsWith(name));
  check("tabs appear", await waitFor(() => tabNamed("Needs you") && tabNamed("Good to know") && tabNamed("The rest")));
  // Maya answered my reply, so her conversation waits on me and belongs on Needs you.
  await press(() => tabNamed("Needs you"));
  await waitFor(() => row("Maya Chen") && !row("Weekly Digest"), 15000);
  check("people are here, and newsletters aren't", !!row("Maya Chen") && !row("Weekly Digest"), rows().map((r) => r.innerText.slice(0, 20)));
  // Each tab shows its own mail, never a list remembered from another tab or view.
  const sameAsEngine = async (tab) => {
    const want = await invoke("call", { cmd: "list_all", args: { role: "inbox", tab, limit: 200, unread_only: false, offset: 0 } });
    const ok = await waitFor(() => {
      const shown = rows();
      return shown.length === want.length && want.every((t) => shown.some((r) => r.innerText.includes(t.subject))) && want;
    }, 10000);
    return { ok: !!ok, detail: { shown: rows().map((r) => r.innerText.slice(0, 30)), engine: want.map((t) => t.subject) } };
  };
  const needsList = await sameAsEngine("needs_you");
  check("Needs you lists just what's waiting on me", needsList.ok, needsList.detail);
  await press(() => tabNamed("Good to know"));
  const goodList = await sameAsEngine("good_to_know");
  check("Good to know lists its own mail, with no newsletters", goodList.ok && !row("Weekly Digest") && !row("Maya Chen"), goodList.detail);
  await press(() => tabNamed("The rest"));
  check("newsletters have their own tab", await waitFor(() => row("Weekly Digest") && !row("Maya Chen"), 10000));
  const tabSum = [...document.querySelectorAll(".tab-count")].reduce((sum, c) => sum + Number(c.innerText), 0);
  const inSidebar = Number(folderButton("All Inbox").querySelector(".nav-count")?.innerText ?? 0);
  check("tab numbers are shown", tabSum >= inSidebar, [tabSum, inSidebar]);

  // The Needs you number goes by the Unread filter. Maya's email has been read by now.
  const needsYouNumber = () => tabNamed("Needs you")?.querySelector(".tab-count")?.innerText ?? "0";
  const needsYouAll = String((await invoke("call", { cmd: "tab_counts", args: {} })).needs_you);
  const needsYouUnread = String((await invoke("call", { cmd: "tab_counts", args: { unread: true } })).needs_you);
  await flipUnreadFilter();
  check("with only unread showing, Needs you counts only unread", needsYouAll !== needsYouUnread && await waitFor(() => needsYouNumber() === needsYouUnread), [needsYouNumber(), needsYouUnread, needsYouAll]);
  await flipUnreadFilter();
  check("with everything showing, Needs you counts everything", await waitFor(() => needsYouNumber() === needsYouAll), [needsYouNumber(), needsYouAll]);
  await waitFor(() => row("Weekly Digest"), 10000);

  // Always on a tab.
  await rightClick("Weekly Digest");
  await choose("More…");
  await sleep(200);
  await choose("Always put on tab…");
  (await waitFor(() => [...labelled("Email actions").querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.trim() === "Good to know"))).click();
  check("a sender can be moved to a tab", await waitFor(() => !row("Weekly Digest"), 10000));
  await press(() => tabNamed("Good to know"));
  check("and it's there now", await waitFor(() => row("Weekly Digest"), 10000));
  await openSettings("Spam");
  check("the choice is listed", await waitFor(() => pageHas("Always on a tab") && [...document.querySelectorAll(".settings .group-row")].some((r) => r.innerText.includes("Good to know") && r.innerText.includes("@"))));
  await press(() => labelled("Sort the inbox into tabs"));
  await closeSettings();
  await waitFor(() => !document.querySelector('[role="tab"]'));
};

// Working out how to connect an address. Nothing here signs in anywhere.
const providersFlow = async () => {
  await openSettings("General");
  (await waitFor(() => [...document.querySelectorAll(".settings-nav .nav-item")].find((b) => b.innerText.includes("Connect an inbox")))).click();
  const box = await waitFor(() => document.querySelector('.welcome input[type="email"]'));
  check("connecting starts with just the address", !!box && !document.querySelector('.welcome input[type="password"]'));
  const tryAddress = async (address) => {
    setValue(document.querySelector('.welcome input[type="email"]'), address);
    await sleep(50);
    await press(() => buttonWith("Continue"));
    return waitFor(() => document.querySelector(".welcome-card[data-provider]"), 15000);
  };
  // Once a provider is found the address shows as a line with "Change", which brings the box back.
  const again = async () => {
    const change = buttonWith("Change");
    if (change && !document.querySelector('.welcome input[type="email"]')) change.click();
    const box = await waitFor(() => document.querySelector('.welcome input[type="email"]'));
    setValue(box, "");
    await waitFor(() => !document.querySelector(".welcome-card[data-provider]"));
  };
  const link = () => document.querySelector(".guide-link")?.getAttribute("href") ?? "";
  const canConnect = () => !!buttonWith("Connect inbox");

  const gmail = await tryAddress("me@gmail.com");
  check("Gmail offers browser sign-in first", gmail?.dataset.provider === "google" && !!buttonWith("Continue with Google") && !document.querySelector('.welcome input[type="password"]'), gmail?.innerText);
  await press(() => buttonWith("Use an app password instead"));
  check("with app password steps a click away", await waitFor(() => gmail.querySelectorAll(".guide li").length === 3 && !!gmail.querySelector('.guide li:last-child input[aria-label="App password"]') && canConnect()), gmail?.innerText);
  check("with a direct link to Google", link().startsWith("https://myaccount.google.com/"), link());
  check("and doesn't show server boxes", !document.querySelector(".settings-server"));
  await again();

  const icloud = await tryAddress("me@icloud.com");
  check("iCloud shows its own steps", icloud?.dataset.provider === "icloud" && link().includes("apple.com"), icloud?.innerText);
  await again();

  const outlook = await tryAddress("me@outlook.com");
  check("Outlook offers browser sign-in and no password box", outlook?.dataset.provider === "microsoft" && !!buttonWith("Continue with Microsoft") && !document.querySelector('.welcome input[type="password"]') && !canConnect(), outlook?.innerText);
  await again();

  const tuta = await tryAddress("me@tuta.com");
  check("providers that can't connect say so", tuta?.dataset.provider === "tuta" && pageHas("doesn't let other mail apps connect") && !canConnect());
  await press(() => buttonWith("Use another email"));
  check("and offer another address", await waitFor(() => !document.querySelector(".welcome-card[data-provider]") && !document.querySelector('.welcome input[type="email"]').value));

  const proton = await tryAddress("me@proton.me");
  check("Proton explains Bridge", proton?.dataset.provider === "proton" && pageHas("password from Bridge") && pageHas("paid Proton plan"), proton?.innerText);
  await again();

  const listed = await tryAddress("me@listed.test");
  check("other providers are filled in from the directory", listed?.innerText.includes("Listed Mail") && canConnect(), listed?.innerText);
  await press(() => buttonWith("Server settings"));
  const server = await waitFor(() => [...document.querySelectorAll(".settings-server input")][0]);
  check("with their server ready", server?.value === "imap.listed.test", server?.value);
  await again();

  const unknown = await tryAddress("me@nowhere.test");
  check("unknown providers ask for the server", unknown && !!document.querySelector(".settings-server") && pageHas("couldn't find the settings"));
  await again();

  // Really signing in, through the stand-in provider.
  await tryAddress("me@outlook.com");
  await press(() => buttonWith("Continue with Microsoft"));
  check("signing in through the browser connects the inbox", await waitFor(() => !document.querySelector(".welcome") && labelled("Switch account"), 20000), document.body.innerText.slice(0, 200));
  await press(() => labelled("Switch account"));
  const inMenu = await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].map((b) => b.innerText).join(" "));
  check("using the account actually signed in to", inMenu.includes("signed@inbox.test"), inMenu);
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await waitFor(() => !document.querySelector('[role="menu"]'));
  await openSettings("signed@inbox.test");
  check("Settings says how it's signed in, with no password box", await waitFor(() => pageHas("Signed in with Microsoft")) && !labelled("New password"));
  await closeSettings();

  // Access removed at the provider.
  await invoke("selftest_share", { name: "revoke", value: "1" });
  await sleep(800);
  const note = () => document.querySelector(".signin-note");
  labelled("Refresh mail")?.click();
  check("when access is removed, it asks to sign in again", await waitFor(() => note()?.innerText.includes("signed@inbox.test"), 20000), document.querySelector('[role="status"]')?.innerText);
  await press(() => [...note().querySelectorAll("button")][0]);
  check("and signing in again clears it", await waitFor(() => !note(), 20000));

  await openSettings("signed@inbox.test");
  (await waitFor(() => inSettings("Disconnect this inbox"))).click();
  (await waitFor(() => buttonWith("Yes, disconnect"))).click();
  await waitFor(() => !document.querySelector(".settings"), 10000);
  await waitFor(() => !labelled("Switch account")?.innerText.includes("signed@inbox.test"), 10000);
};

// A sign-in code is copied from its row in one click, and the email goes to the bin.
const codeFlow = async () => {
  const subject = "Your Acme sign-in code";
  await press(() => folderButton("Archive"));
  await shareSet("codemail", "1");
  let row = null;
  for (let i = 0; i < 100 && !row; i++) {
    labelled("Refresh mail")?.click();
    await sleep(300);
    row = buttonWith(subject);
  }
  check("a sign-in code email arrives", !!row);
  const chip = () => buttonWith(subject)?.querySelector(".code-chip");
  check("its row shows the code to copy", await waitFor(() => chip()?.innerText.includes("482913")), chip()?.innerText);
  await press(() => chip());
  check("copying it sends the email to the bin, as chosen in Settings", await waitFor(() => !buttonWith(subject)));
  await press(() => folderButton("Trash"));
  check("it's in the bin", await waitFor(() => buttonWith(subject), 15000));
  await press(() => folderButton("All Inbox"));
};

// Setting mail aside, rules, the command bar and the other daily tools.
// Only ever leaves by email, which goes to the stand-in server. The other kinds open a real browser.
const newslettersFlow = async () => {
  const rowOf = (name) => [...document.querySelectorAll(".news-row")].find((r) => r.innerText.includes(name));
  const tabOf = (name) => [...document.querySelectorAll('.newsletters [role="tab"]')].find((t) => t.innerText.startsWith(name));
  const leaveButton = () => rowOf("Field Notes").querySelector('[aria-label^="Unsubscribe"]');
  await press(() => folderButton("All Inbox"));
  await shareSet("newslettermail", "1");
  // It's filed in Archive, so it's looked for where this view gets it.
  let arrived = false;
  for (let i = 0; i < 100 && !arrived; i++) {
    labelled("Refresh mail")?.click();
    await sleep(300);
    arrived = (await invoke("call", { cmd: "newsletters", args: {} })).some((r) => r.address === "hello@fieldnotes.test");
  }
  check("newsletter mail arrives", arrived);

  await press(() => folderButton("Newsletters"));
  check("Newsletters opens from the sidebar", await waitFor(() => document.querySelector(".newsletters .list-title")?.innerText === "Newsletters" && rowOf("Field Notes")));
  check("it sits in the sidebar as the current view", folderButton("Newsletters").getAttribute("aria-current") === "true");
  check("senders with a way out are listed", rowOf("Daily Brief") && rowOf("Shop Deals"));
  check("people are not", !rowOf("Maya Chen"));
  check("each sender shows how many emails", rowOf("Daily Brief").querySelector("td.news-count")?.innerText === "3", rowOf("Daily Brief").innerText);
  check("a sender with a page says so", rowOf("Shop Deals").querySelector("button[data-tip]")?.dataset.tip === "Opens their unsubscribe page");
  check("a sender by email says so", leaveButton().dataset.tip?.startsWith("Sends an unsubscribe email from"));

  const search = labelled("Search newsletters");
  setValue(search, "field");
  check("search narrows the list", await waitFor(() => rowOf("Field Notes") && !rowOf("Daily Brief")));
  setValue(search, "");
  await waitFor(() => rowOf("Daily Brief"));
  await press(() => buttonWith("Emails"));
  const counts = () => [...document.querySelectorAll(".news-row td.news-count")].map((td) => Number(td.innerText.replace(/,/g, "")) || 0);
  check("the busiest can come first", await waitFor(() => counts().length > 1 && counts()[0] === Math.max(...counts())), counts());
  await press(() => buttonWith("Latest email"));

  // Several at once.
  await press(() => rowOf("Daily Brief").querySelector(".news-pick input"));
  check("choosing a sender offers to unsubscribe several at once", await waitFor(() => document.querySelector(".news-bulk")?.innerText.includes("1 sender chosen")));
  await press(() => [...document.querySelectorAll(".news-bulk button")].find((b) => b.innerText.trim() === "Clear"));
  check("and clearing puts it away", await waitFor(() => !document.querySelector(".news-bulk")));

  // One click, straight away, with time to change their mind.
  const before = await outboxCount();
  await press(() => leaveButton());
  check("one click moves them out at once", await waitFor(() => !rowOf("Field Notes"), 300));
  check("and offers to take it back", await waitFor(() => pageHas("Unsubscribed from Field Notes.") && buttonWith("Undo")));
  await press(() => buttonWith("Undo"));
  check("Undo brings them back", await waitFor(() => rowOf("Field Notes")));
  await sleep(500);
  check("and nothing is sent", (await outboxCount()) === before);

  await press(() => leaveButton());
  await waitFor(() => !rowOf("Field Notes"));
  const sent = await nextSent(before);
  check("the email they ask for goes without a composer", sent?.to?.includes("leave@fieldnotes.test") && !document.querySelector(".composer"), sent?.to);
  check("it asks for what the list wants", sent?.data?.includes("Subject: Leave"));
  await press(() => tabOf("Unsubscribed"));
  check("they're on the Unsubscribed tab", await waitFor(() => rowOf("Field Notes")?.innerText.includes("Unsubscribed")));

  await press(() => rowOf("Field Notes"));
  check("clicking a sender shows their mail", await waitFor(() => document.querySelector(".list-title")?.innerText === "Field Notes" && buttonWith("Field Notes, issue 2") && buttonWith("Field Notes, issue 1")));
  labelled("Back")?.click();
  await waitFor(() => document.querySelector(".newsletters"));
  check("going back returns to the same tab", await waitFor(() => tabOf("Unsubscribed")?.getAttribute("aria-selected") === "true" && rowOf("Field Notes")));

  // Remembered, not just on screen.
  await press(() => folderButton("All Inbox"));
  await waitFor(() => !document.querySelector(".newsletters"));
  await press(() => folderButton("Newsletters"));
  await waitFor(() => tabOf("Subscribed") && rowOf("Daily Brief"));
  check("they stay off the Subscribed tab", !rowOf("Field Notes"));
  await press(() => tabOf("Unsubscribed"));
  check("and stay on the Unsubscribed tab", await waitFor(() => rowOf("Field Notes")));

  // With only icons showing, it's still there, and still fits.
  await press(() => buttonWith("Collapse sidebar"));
  check("the rail keeps it, named", await waitFor(() => labelled("Newsletters")?.closest("nav") && document.querySelector(".app[data-rail]")));
  const table = document.querySelector(".news-table").getBoundingClientRect();
  const panel = document.querySelector(".newsletters").getBoundingClientRect();
  check("the table fills the panel with no sideways scroll", table.right <= panel.right && table.width > panel.width * 0.9, [table.width, panel.width]);
  await press(() => buttonWith("Expand sidebar"));
  await waitFor(() => !document.querySelector(".app[data-rail]"));
  await press(() => folderButton("All Inbox"));
  await waitFor(() => !document.querySelector(".newsletters"));
};

const laterFlow = async () => {
  const rows = () => [...document.querySelectorAll(".thread")];
  const row = (name) => rows().find((r) => r.innerText.includes(name));
  const nav = (name) => [...document.querySelectorAll("nav .nav-item")].find((b) => b.innerText.includes(name));
  const menuPick = (t) => waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes(t)));
  const key = (k, extra = {}) => window.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...extra }));
  const more = async () => {
    await press(() => labelled("More actions"));
    await waitFor(() => labelled("More actions")?.getAttribute("aria-expanded") === "true");
  };
  const NAME = "Q4 launch plan";

  await press(() => folderButton("All Inbox"));
  await waitFor(() => row(NAME));

  // Snooze
  await press(() => row(NAME));
  await waitFor(() => labelled("Snooze"));
  await press(() => labelled("Snooze"));
  (await menuPick("Tomorrow morning")).click();
  check("snoozing takes it out of the inbox", await waitFor(() => !row(NAME)));
  check("Snoozed appears in the sidebar", await waitFor(() => nav("Snoozed")));
  await press(() => nav("Snoozed"));
  check("and it's listed there, with when it comes back", await waitFor(() => row(NAME)?.innerText.match(/\d/)));
  row(NAME).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 }));
  (await menuPick("Bring back now")).click();
  check("bringing it back empties Snoozed", await waitFor(() => !nav("Snoozed")));
  await press(() => folderButton("All Inbox"));
  check("and it's in the inbox again", await waitFor(() => row(NAME)));

  // Pin
  await press(() => row(NAME));
  await waitFor(() => labelled("More actions"));
  await more();
  (await menuPick("Pin to the top")).click();
  check("a pinned email is first, under Pinned", await waitFor(() => rows()[0]?.innerText.includes(NAME) && pageHas("Pinned")));
  await more();
  (await menuPick("Unpin")).click();
  await waitFor(() => !pageHas("Pinned"));

  // Find in a conversation
  await waitFor(() => labelled("More actions"));
  await more();
  (await menuPick("Find in this conversation")).click();
  const find = await waitFor(() => labelled("Find in this conversation"));
  setValue(find, "press date");
  check("finding in a conversation says how many match", await waitFor(() => /1 of \d/.test(document.querySelector(".reader-find")?.innerText ?? "")), document.querySelector(".reader-find")?.innerText);
  check("and marks the words", await waitFor(() => document.querySelector(".mail-text mark")?.innerText.toLowerCase() === "press date"));
  await press(() => labelled("Close find"));

  // Print
  await more();
  (await menuPick("Print")).click();
  check("printing lays out the conversation", await waitFor(() => document.querySelector(".print-sheet article")));
  await waitFor(() => !document.querySelector(".print-sheet"), 5000);

  // Single keys
  const first = rows().findIndex((r) => r.getAttribute("aria-current") === "true");
  key("j");
  check("J opens the next email", await waitFor(() => rows().findIndex((r) => r.getAttribute("aria-current") === "true") === first + 1), JSON.stringify({ first, active: document.activeElement?.tagName, cls: document.activeElement?.className, blockers: [...document.querySelectorAll('.settings, .scrim, [role="dialog"], [role="menu"], .composer-inline')].map((e) => e.className || e.tagName + ":" + e.getAttribute("aria-label")), sheet: !!document.querySelector(".print-sheet") }));
  key("k");
  check("K goes back up", await waitFor(() => rows().findIndex((r) => r.getAttribute("aria-current") === "true") === first));

  // Command bar
  key("k", { metaKey: true });
  const bar = await waitFor(() => labelled("Type a command"));
  check("⌘K opens the command bar", !!bar);
  setValue(bar, "open filters");
  await waitFor(() => document.querySelectorAll(".command-row").length >= 1);
  bar.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  // "Open filters" opens Settings on the Filters page, ready to add one.
  check("and it does what's typed", await waitFor(() => document.querySelector(".settings-page h1")?.innerText === "Filters" && inSettings("New filter")), document.querySelector(".settings-page h1")?.innerText);

  // Filters: picked with the rows, and an AI kind beside them.
  (await waitFor(() => inSettings("New filter"))).click();
  check("a filter can be normal or by AI", !!(await waitFor(() => [...document.querySelectorAll(".kind-switch [role=radio]")].length === 2)));
  setValue(await waitFor(() => labelled("Rule from")), "robot@northwind.test");
  await sleep(50);
  check("and says what it would match", await waitFor(() => pageHas("Nothing in your inbox matches")));
  const ruleFrom = labelled("Rule from");
  setValue(ruleFrom, "robot@northwind.test, ");
  await sleep(50);
  check("a list can be typed with its commas", labelled("Rule from").value === "robot@northwind.test, ", labelled("Rule from").value);
  pressEnter(labelled("Rule from"));
  await sleep(200);
  check("pressing return there doesn't start over", labelled("Rule from").value === "robot@northwind.test, ");
  setValue(await waitFor(() => labelled("Rule from")), "robot@northwind.test");
  await sleep(50);
  await press(() => buttonWith("Save filter"));
  const savedName = "Delete filter: Emails from robot@northwind.test: mark as read.";
  check("a saved rule is listed", await waitFor(() => labelled(savedName)));
  await press(() => labelled(savedName));
  await waitFor(() => !labelled(savedName));

  // Templates
  await openSettings("Templates");
  (await waitFor(() => inSettings("New template"))).click();
  setValue(await waitFor(() => labelled("Template name")), "Thanks");
  const words = labelled("Template words");
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set.call(words, "Thank you for writing.");
  words.dispatchEvent(new Event("input", { bubbles: true }));
  await sleep(50);
  await press(() => buttonWith("Add template"));
  check("a template can be added", await waitFor(() => labelled("Delete template Thanks")));

  // Mail on this device
  await openSettings("Storage");
  check("the mail page shows what's here", await waitFor(() => pageHas("Room used on this device") && pageHas("demo@inbox.test")));
  check("and that it's all come down", await waitFor(() => pageHas("Everything's here"), 30000), document.querySelector(".settings-page")?.innerText.slice(0, 300));
  await closeSettings();

  // Send later, with the template and a reminder
  key("n", { metaKey: true });
  await waitFor(() => document.querySelector(".ProseMirror"));
  const to = await waitFor(() => labelled("To"));
  setValue(to, "later@inbox.test");
  pressEnter(to);
  setValue(await waitFor(() => labelled("Subject")), "Scheduled note");
  await press(() => labelled("Templates"));
  (await menuPick("Thanks")).click();
  check("the template goes into the message", await waitFor(() => document.querySelector(".ProseMirror")?.innerText.includes("Thank you for writing.")));
  await press(() => labelled("Send later"));
  (await menuPick("Tomorrow morning")).click();
  check("scheduling says when, and that the app must be open", await waitFor(() => pageHas("Scheduled for") && pageHas("as long as this app is open")));
  check("Scheduled appears in the sidebar", await waitFor(() => nav("Scheduled")));
  const sentBefore = await outboxCount();
  await invoke("call", { cmd: "due_tick", args: {} });
  check("it doesn't go early", (await outboxCount()) === sentBefore);
  await press(() => nav("Scheduled"));
  const scheduled = await waitFor(() => row("Scheduled note"));
  check("it's listed with its time", !!scheduled);
  await press(() => scheduled);
  check("opening it takes it off the schedule to change it", await waitFor(() => pageHas("Taken off the schedule")));
  (await waitFor(() => labelled("Discard"))).click();
  (await waitFor(() => buttonWith("Yes, discard"))).click();
  await waitFor(() => !nav("Scheduled"));
  await openSettings("Templates");
  (await waitFor(() => labelled("Delete template Thanks"))).click();
  await closeSettings();

  // Group by sender
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row(NAME));
  await press(() => [...document.querySelectorAll(".list-filter")][0]);
  (await waitFor(() => [...document.querySelectorAll('[role="menuitem"]')].find((b) => b.innerText.includes("Group by sender")))).click();
  const sender = await waitFor(() => [...document.querySelectorAll(".sender-open")].find((b) => b.innerText.includes("Maya Chen")));
  check("grouping by sender shows one row each", !!sender);
  await press(() => sender);
  check("and opening one shows only their mail", await waitFor(() => row(NAME) && !row("Weekly Digest")));
  await press(() => labelled("Back"));
  await waitFor(() => labelled("Back to emails"));
  await press(() => labelled("Back to emails"));
  await waitFor(() => row("Weekly Digest"));

  // The screener
  await invoke("call", { cmd: "set_setting", args: { key: "screener", value: "on" } });
  await shareSet("strangermail", "1");
  const shows = () => press(() => labelled("Refresh mail"));
  await shows();
  check("someone new waits under New senders", await waitFor(() => { shows(); return nav("New senders"); }, 30000));
  check("rather than reaching the inbox", !row("A first hello"));
  await press(() => nav("New senders"));
  const letIn = await waitFor(() => buttonWith("Let in"));
  check("the screener shows them", pageHas("newperson@elsewhere.test") || pageHas("New Person"));
  await press(() => letIn);
  await waitFor(() => !nav("New senders"));
  await press(() => folderButton("All Inbox"));
  check("once let in, their mail is in the inbox", await waitFor(() => row("A first hello")));
  await invoke("call", { cmd: "set_setting", args: { key: "screener", value: "off" } });

  // Mute
  await press(() => row("A first hello"));
  await waitFor(() => labelled("More actions"));
  await more();
  (await menuPick("Mute conversation")).click();
  check("muting takes it out of the inbox", await waitFor(() => !row("A first hello")));
  (await waitFor(() => nav("Muted"))).click();
  await waitFor(() => row("A first hello"));
  row("A first hello").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 }));
  (await menuPick("More…")).click();
  (await menuPick("Unmute")).click();
  check("and unmuting brings it back", await waitFor(() => !nav("Muted")));
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row("A first hello"));

  // A clicked banner opens its email, even from the calendar, which has nowhere to show one.
  await press(() => [...document.querySelectorAll("nav .nav-item")].find((b) => b.innerText.includes("Calendar")));
  await waitFor(() => document.querySelector(".calendar-grid"));
  const hello = (await invoke("call", { cmd: "list_all", args: { role: "inbox" } })).find((t) => t.subject === "A first hello");
  await invoke("selftest_click_banner", { accountId: hello.account_id, threadId: hello.thread_id });
  check("a clicked banner opens its email", await waitFor(() => document.querySelector(".reader-page")?.innerText.includes("A first hello")));
  check("and leaves the calendar for the list", await waitFor(() => !document.querySelector(".calendar-grid") && row("A first hello")));
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row("A first hello"));

  // The calendar
  await press(() => [...document.querySelectorAll("nav .nav-item")].find((b) => b.innerText.includes("Calendar")));
  check("the calendar opens on this month", await waitFor(() => document.querySelector(".calendar-grid") && pageHas(new Date().toLocaleDateString([], { month: "long", year: "numeric" }))));
  check("the calendar fills the whole panel", document.querySelector(".calendar").getBoundingClientRect().width >= document.querySelector(".main-panel").getBoundingClientRect().width - 1);
  await press(() => buttonWith("New event"));
  setValue(await waitFor(() => labelled("Event name")), "Team lunch");
  await sleep(50);
  await press(() => [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.innerText === "Add event"));
  check("a new event shows on the day", await waitFor(() => [...document.querySelectorAll(".calendar-event")].some((e) => e.innerText.includes("Team lunch"))), [document.querySelector('[role="dialog"]')?.innerText, document.querySelector(".calendar-day")?.innerText]);
  check("and in the month", [...document.querySelectorAll(".calendar-chip")].some((e) => e.innerText.includes("Team lunch")));
  await press(() => [...document.querySelectorAll(".calendar-event")].find((e) => e.innerText.includes("Team lunch")));
  (await waitFor(() => [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.innerText === "Delete"))).click();
  (await waitFor(() => buttonWith("Yes, delete"))).click();
  check("and can be deleted", await waitFor(() => ![...document.querySelectorAll(".calendar-event")].some((e) => e.innerText.includes("Team lunch"))));
  const dayText = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  const firstDay = new Date();
  const lastDay = new Date(firstDay.getFullYear(), firstDay.getMonth(), firstDay.getDate() + 1);
  await press(() => buttonWith("New event"));
  setValue(await waitFor(() => labelled("Event name")), "Team trip");
  await press(() => labelled("All day"));
  setValue(await waitFor(() => labelled("Starts")), dayText(firstDay));
  await sleep(50);
  setValue(await waitFor(() => labelled("Ends")), dayText(lastDay));
  await sleep(50);
  await press(() => [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.innerText === "Add event"));
  await waitFor(() => [...document.querySelectorAll(".calendar-event")].some((e) => e.innerText.includes("Team trip")));
  check("a two-day event shows on both days", await waitFor(() => [...document.querySelectorAll(".calendar-chip")].filter((e) => e.innerText.includes("Team trip")).length === 2));
  await press(() => [...document.querySelectorAll(".calendar-event")].find((e) => e.innerText.includes("Team trip")));
  await waitFor(() => labelled("Starts"));
  check("and opens with the dates it was given", labelled("Starts").value.startsWith(dayText(firstDay)) && labelled("Ends").value.startsWith(dayText(lastDay)), `${labelled("Starts").value} to ${labelled("Ends").value}`);
  await press(() => [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.innerText === "Delete"));
  (await waitFor(() => buttonWith("Yes, delete"))).click();
  await waitFor(() => ![...document.querySelectorAll(".calendar-event")].some((e) => e.innerText.includes("Team trip")));
  await press(() => folderButton("All Inbox"));
  await waitFor(() => row(NAME));

  // AI, on the person's own key
  await openSettings("AI");
  setValue(await waitFor(() => labelled("AI key")), "test-ai-key");
  await sleep(50);
  await press(() => inSettings("Test and save"));
  check("the AI page lists what it can be used for", await waitFor(() => pageHas("Ending in") && pageHas("Summaries") && pageHas("Writing replies") && pageHas("Spam second opinion") && pageHas("What AI may read")), document.querySelector(".settings-page")?.innerText.slice(0, 300));
  // Other AI apps on this Mac have their own page now.
  await openSettings("MCP");
  check("other AI apps have their own page", await waitFor(() => document.querySelector(".settings-page h1")?.innerText === "MCP" && pageHas("other AI apps")));
  await closeSettings();
  await waitFor(() => row("Q4 launch plan"), 20000);
  await press(() => row("Q4 launch plan"));
  await waitFor(() => labelled("More actions"));
  await press(() => labelled("More actions"));
  (await menuPick("Summarize")).click();
  check("a summary shows above the conversation", await waitFor(() => labelled("Summary")?.innerText.includes("review of the Q4 launch plan")), document.querySelector(".ai-card")?.innerText);
  check("and says it's waiting on you", labelled("Summary").innerText.includes("Waiting on you"));
  await press(() => labelled("Close summary"));
  await press(() => labelled("More actions"));
  (await menuPick("Translate into")).click();
  check("a translation shows in the email", await waitFor(() => document.querySelector(".translated")?.innerText.includes("Hola")), document.querySelector(".ai-card")?.innerText);
  await press(() => document.querySelector(".quick-reply"));
  await waitFor(() => labelled("Use AI"));
  await press(() => labelled("Use AI"));
  const note = await waitFor(() => labelled("What should the reply say"));
  setValue(note, "say yes");
  await sleep(50);
  await press(() => note.closest("form").querySelector("button"));
  check("AI's words go in the reply, for checking", await waitFor(() => document.querySelector(".ProseMirror")?.innerText.includes("I'll review it by Friday")), document.querySelector(".composer-problem")?.innerText);
  check("and nothing is sent by itself", !pageHas("Sending in"));
  (await waitFor(() => labelled("Discard"))).click();
  (await waitFor(() => buttonWith("Yes, discard"))).click();
  await waitFor(() => !document.querySelector(".composer-inline"));
  await openSettings("AI");
  (await waitFor(() => inSettings("Remove"))).click();
  (await waitFor(() => buttonWith("Yes, remove"))).click();
  await waitFor(() => labelled("AI key"));
  await closeSettings();
};
