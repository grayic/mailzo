// The second device: signs in and waits to be approved.
guarded(async () => {
  await shareWait("aReady", 480000);
  check("a new computer starts at connect", await waitFor(() => pageHas("Welcome to Mailzo")));
  await press(() => buttonWith("Already have an account"));
  await waitFor(() => buttonWith("Email me a code"));

  // Business addresses are welcome.
  setValue(document.querySelector('input[type="email"]'), "hello@grayic.test");
  await sleep(50);
  await press(() => buttonWith("Email me a code"));
  check("business addresses can sign in", await waitFor(() => pageHas("We sent a 6-digit code to hello@grayic.test")));
  await press(() => buttonWith("Change email"));

  const { code, input } = await signInWith("demo@inbox.test");
  check("sign-in code arrives", !!code && !!input);
  setValue(input, code === "000000" ? "000001" : "000000");
  await sleep(50);
  await press(() => buttonWith("Continue"));
  check("wrong code shows a clear message", await waitFor(() => pageHas("wrong or has expired")));
  await press(() => buttonWith("Change email"));

  // Google's sign-in reaches the same account, and still needs approval.
  (await waitFor(() => buttonWith("Continue with Google"))).click();
  check("second device waits for approval, with no mail", await waitFor(() => pageHas("Allow this device")));
  const accounts = await invoke("call", { cmd: "list_accounts", args: {} });
  check("no inboxes before approval", accounts.length === 0, accounts.length);

  const shown = await waitFor(() => { const m = dialog("Number your other device shows")?.innerText.trim(); return /^\d{6}$/.test(m) ? m : null; }, 30000);
  check("number appears on the new device", !!shown);
  await shareSet("number", shown);

  check("after approval the inbox and its mail appear", await waitFor(() => pageHas("Weekly Digest") && pageHas("Maya Chen"), 40000));
  check("the password really arrived: sync has no errors", !document.querySelector('[role="status"]'), document.querySelector('[role="status"]')?.innerText);
  await press(() => buttonWith("Maya Chen"));
  check("mail opens on the new device", await waitFor(() => document.querySelectorAll("article").length === 3 && pageHas("Final version goes out on Tuesday")), [document.querySelectorAll("article").length, document.querySelector("section h1")?.innerText, document.querySelector(".pane-reader")?.innerText.slice(0, 300)]);
  await shareSet("bHasMail", "1");

  check("being removed returns to the start", await waitFor(() => pageHas("Welcome to Mailzo"), 40000));
  const after = await invoke("call", { cmd: "list_accounts", args: {} });
  check("being removed erases the inboxes here", after.length === 0, after.length);
  await shareSet("bSignedOut", "1");
});
