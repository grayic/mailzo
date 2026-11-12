// Just the daily tools, for working on them. The full test runs them too.
guarded(async () => {
  await waitFor(() => pageHas("Welcome to Mailzo"));
  await connectDemoInbox("demo-password");
  await waitFor(() => pageHas("Maya Chen"), 30000);
  await setUpAccount();
  await waitFor(() => dialog("Recovery key"), 20000);
  await press(() => document.querySelector('input[type="checkbox"]'));
  await sleep(50);
  await press(() => buttonWith("Continue"));
  await waitFor(() => document.querySelector('[role="tab"]'));
  await openSettings("Spam");
  await press(() => dialog("Sort the inbox into tabs"));
  await closeSettings();
  await waitFor(() => pageHas("Maya Chen"), 20000);
  await laterFlow();
  await newslettersFlow();
  await codeFlow();
});
