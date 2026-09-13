import { test, expect, type Page } from "@playwright/test";

import { mock, imported } from "./helpers/library";

test("empty library, sequential imports with partial failure, search and restart persistence", async ({
  page,
}) => {
  await mock(page);
  await expect(
    page.getByRole("heading", { name: "Your library starts here" }),
  ).toBeVisible();
  await page.evaluate(() => {
    (window as any).__files = [
      "/test/One.ipa",
      "/test/Bad.ipa",
      "/test/Two.ipa",
    ];
  });
  await page.getByRole("button", { name: "Import IPA", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "Finished importing 3" }),
  ).toBeVisible();
  await expect(
    page.getByRole("list", { name: "Import results" }),
  ).toContainText("Invalid IPA");
  await expect(
    page.getByRole("link").filter({ hasText: "2 saved versions" }),
  ).toBeVisible();
  await page.getByRole("searchbox", { name: "Search apps" }).fill("missing");
  await expect(page.getByText("No apps match")).toBeVisible();
  await page.getByRole("searchbox").fill("test.library");
  await page.locator(".library-row").click();
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toHaveCount(2);
  await expect(page.locator(".library-version").first()).toContainText("Saved");
  await page.reload();
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toHaveCount(2);
  expect(
    await page.evaluate(() =>
      (window as any).__calls.some(
        (c: any) => c.cmd.includes("sign") && c.cmd !== "account_status",
      ),
    ),
  ).toBe(false);
});

test("saved version workspace preserves form state across navigation and duplicate imports", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Install an app" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Re-sign with my Apple account" })
    .click();
  await page.getByLabel("Apple account email").fill("local@example.invalid");
  await page.getByRole("link", { name: "App Library", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "App Library", exact: true }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Resume Library App" }).click();
  await expect(page.getByLabel("Apple account email")).toHaveValue(
    "local@example.invalid",
  );
  await page.getByRole("link", { name: "Back to library" }).click();
  await page.getByRole("button", { name: "Import IPA", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Install an app" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Back to library" }).click();
  await expect(
    page.getByRole("list", { name: "Import results" }),
  ).toContainText("Already saved");
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toHaveCount(1);
});

test("an unrecognised sign-in failure offers its technical detail, and only then", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Re-sign with my Apple account" })
    .click();

  // Nothing has failed yet, so there is nothing to report.
  await expect(page.getByText("Technical details")).toHaveCount(0);

  await page.evaluate(() => {
    (window as any).__signInFailure = {
      stage: "failed",
      account: null,
      selected_team: null,
      challenge: null,
      teams: [],
      message:
        "Apple account authentication or two-factor verification failed. Apple's sign-in response did not carry the password-verification fields this step needs.",
      diagnostic:
        "Failed to parse initial login response ← AuthWithMessage(-22320)",
    };
  });
  await page.getByLabel("Apple account email").fill("local@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByRole("checkbox", { name: /I agree to authenticate/ })
    .setChecked(true);
  await page.getByRole("button", { name: "Sign in to Apple" }).click();

  await expect(
    page.getByText("did not carry the password-verification fields"),
  ).toBeVisible();
  // The detail is folded away until asked for: it is evidence, not advice.
  const details = page.getByText("Failed to parse initial login response ←");
  await expect(details).not.toBeVisible();
  await page.getByText("Technical details").click();
  await expect(details).toBeVisible();
  await expect(page.getByText("AuthWithMessage(-22320)")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Copy diagnostics" }),
  ).toBeVisible();
});

test("the team's membership is named on the account, and an unestablished one says what happens", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Re-sign with my Apple account" })
    .click();
  await page.evaluate(() => {
    (window as any).__teams = [
      {
        id: "PAID1",
        name: "Synthetic Company",
        kind: "Company",
        free: false,
        membership: "Apple Developer Program",
      },
      {
        id: "UNK1",
        name: "Synthetic Unknown",
        kind: "Company",
        free: null,
        membership: null,
      },
    ];
  });
  await page.getByLabel("Apple account email").fill("local@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByRole("checkbox", { name: /I agree to authenticate/ })
    .setChecked(true);
  await page.getByRole("button", { name: "Sign in to Apple" }).click();

  // Two teams, so neither is chosen for the person. The membership is legible before choosing.
  const select = page.getByLabel("Signing team", { exact: true });
  await expect(select).toContainText("Apple Developer Program");
  await expect(select).toContainText("Membership not established");

  // The chosen team's membership stays legible once the step is collapsed, not only inside the
  // list, so it is still answerable later in the flow.
  await select.selectOption("PAID1");
  await expect(page.locator(".stage-summary").first()).toContainText(
    "Apple Developer Program",
  );
  // Reopening the step is how its consequences are read after the fact.
  await page.getByRole("button", { name: "Change Signing team" }).click();
  await expect(page.getByText("Paid membership:")).toBeVisible();

  // Undetermined is a third answer, and it says which limits Orbiter will actually apply rather
  // than leaving a person to guess.
  await select.selectOption("UNK1");
  await expect(page.getByLabel("Signing team", { exact: true })).toHaveValue(
    "UNK1",
  );
  await expect(
    page.getByText("Orbiter applies the stricter free-team limits"),
  ).toBeVisible();
});

test("retained signed build requires review, records exact artifact, and survives restart", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => (window as any).__addSigned());
  await page
    .getByRole("link", { name: "Review installation", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Choose an app and iPhone" }),
  ).toBeVisible();
  await expect(page.getByLabel("Apple account email")).not.toBeVisible();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  const install = page.getByRole("button", { name: "Install on iPhone" });
  await expect(install).toBeDisabled();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await install.click();
  await expect(
    page.getByText("Installation complete", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.find(
          (c: any) => c.cmd === "library_prepare_install",
        ).args,
    ),
  ).toEqual({ artifactId: "signed-1", deviceId: 1 });
  await page.getByRole("link", { name: "Back to library" }).click();
  await page
    .getByRole("button", { name: "Installations", exact: true })
    .click();
  await expect(page.locator(".library-version")).toContainText(
    "Artifact: signed-1",
  );
  await expect(page.locator(".library-version")).toContainText(
    "Installed signed build: Expires",
  );
  await page.reload();
  await page
    .getByRole("button", { name: "Installations", exact: true })
    .click();
  await expect(page.locator(".library-version")).toContainText(
    "My iPhone · installed",
  );
  await page.getByRole("button", { name: "Devices", exact: true }).click();
  await expect(page.locator(".library-version")).toContainText("My iPhone");
});

test("browsing during an installation cannot switch artifacts or remove files", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => {
    (window as any).__addSigned();
    (window as any).__holdInstall = true;
  });
  await page
    .getByRole("link", { name: "Review installation", exact: true })
    .click();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install on iPhone" }).click();
  await page.getByRole("link", { name: "Back to library" }).click();
  await expect(
    page.getByRole("button", { name: "Remove app and history" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toHaveAttribute("aria-disabled", "true");
  await page.getByRole("link", { name: /Installation in progress/ }).click();
  await expect(
    page.getByRole("heading", { name: "Installing on your iPhone" }),
  ).toBeVisible();
  await page.evaluate(() => (window as any).__finishInstall());
  await expect(
    page.getByText("Installation complete", { exact: true }),
  ).toBeVisible();
});

test("version deletion retains history; whole-app removal explicitly removes history", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install on iPhone" }).click();
  await expect(
    page.getByText("Installation complete", { exact: true }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Back to library" }).click();

  await page
    .getByRole("button", { name: "Remove version", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "Installation history is preserved",
  );
  await page.getByRole("button", { name: "Keep files" }).click();
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Remove version", exact: true })
    .click();
  await page.getByRole("button", { name: "Confirm removal" }).click();
  await expect(
    page.getByText("No saved files.", { exact: false }),
  ).toBeVisible();

  await page
    .getByRole("button", { name: "Installations", exact: true })
    .click();
  await expect(page.locator(".library-version")).toContainText(
    "Saved file removed; history retained.",
  );
  await page.getByRole("button", { name: "Remove app and history" }).click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "all saved files and installation history",
  );
  await page.getByRole("button", { name: "Confirm removal" }).click();
  await expect(
    page.getByRole("heading", { name: "Your library starts here" }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Your library starts here" }),
  ).toBeVisible();
});

for (const theme of ["light", "dark"] as const)
  test(`library ${theme} layout supports long names and keyboard at minimum width`, async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: theme });
    await page.setViewportSize({ width: 780, height: 600 });
    await mock(page);
    await imported(page);
    await page.evaluate(() => {
      (window as any).__library.apps[0].name =
        "An extremely long app name ".repeat(12);
      window.dispatchEvent(new Event("library-changed"));
    });
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    await page.getByRole("button", { name: "Versions", exact: true }).focus();
    await page.keyboard.press("Tab");
    await expect(
      page.getByRole("button", { name: "Installations", exact: true }),
    ).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(
      page.getByText("No installation attempts recorded."),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    await page.screenshot({
      path: `test-results/library-${theme}.png`,
      fullPage: true,
    });
  });

test("corrupt library is shown as an error without an empty-library reset", async ({
  page,
}) => {
  await mock(page);
  await page.evaluate(() => {
    (window as any).__corrupt = true;
    window.dispatchEvent(new Event("library-changed"));
  });
  await expect(
    page.getByRole("alert").filter({ hasText: "Library storage is corrupt" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Your library starts here" }),
  ).not.toBeVisible();
});

test("expiration distinguishes saved profiles, successful installs, failures and legacy records", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => {
    const w = window as any;
    const d = w.__library;
    const source = d.artifacts[0];
    source.expires = "2020-01-01T00:00:00Z";
    d.devices = [
      { id: "device-one", name: "First phone", last_seen_unix: 100 },
      { id: "device-two", name: "Second phone", last_seen_unix: 101 },
    ];
    const base = {
      app_id: source.app_id,
      artifact_id: source.id,
      app_name: source.name,
      identifier: source.identifier,
      version: source.version,
      build: source.build,
      sha256: source.sha256,
      signed: true,
      started_unix: 100,
      finished_unix: 101,
      message: "Recorded result",
    };
    d.attempts = [
      {
        ...base,
        id: "expired",
        device_id: "device-one",
        stage: "installed",
        expires: "2020-01-01T00:00:00Z",
      },
      {
        ...base,
        id: "failed",
        device_id: "device-two",
        stage: "failed",
        expires: "2099-01-01T00:00:00Z",
      },
      {
        ...base,
        id: "unknown-expiry",
        device_id: "device-one",
        stage: "installed",
        expires: null,
      },
    ];
    w.__legacyRenewal = {
      sentence: "Older build expires in two days.",
      urgent: false,
      standing: { state: "valid", days: 2 },
      bearing: "other_team",
    };
    window.dispatchEvent(new Event("library-changed"));
    window.dispatchEvent(new Event("focus"));
  });
  await expect(page.locator(".library-version")).toContainText(
    "Imported profile: Expired",
  );
  await page
    .getByRole("button", { name: "Installations", exact: true })
    .click();
  await expect(
    page
      .locator(".library-version")
      .filter({ hasText: "First phone · installed" })
      .filter({ hasText: "Installed signed build: Expired" }),
  ).toHaveCount(1);
  await expect(
    page
      .locator(".library-version")
      .filter({ hasText: "Second phone · failed" }),
  ).toContainText("No installed expiration established");
  await expect(
    page.locator(".library-version").filter({ hasText: "Expiration unknown" }),
  ).toHaveCount(1);
  await page.getByRole("link", { name: "All apps" }).click();
  // The file an older Orbiter wrote is still shown, still labelled as what it is, and — unlike
  // before — can now actually be cleared: this is the only Forget button left in the app.
  await expect(
    page.getByText("Legacy renewal information.", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByText("Older build expires in two days.", { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Forget" }).click();
  await expect
    .poll(async () =>
      page.evaluate(() =>
        ((window as any).__calls as any[]).some(
          (c) => c.cmd === "renewal_forget",
        ),
      ),
    )
    .toBe(true);
});

test("native drag and drop imports multiple files, skips invalid files, and ignores hidden library pages", async ({
  page,
}) => {
  await mock(page);
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__calls.some(
          (c: any) =>
            c.cmd === "plugin:event|listen" &&
            c.args.event === "tauri://drag-drop",
        ),
      ),
    )
    .toBe(true);
  await page.evaluate(() => (window as any).__drag("enter", ["/test/One.ipa"]));
  await expect(page.locator(".library-drop-zone")).toHaveText(
    "Drop your IPA files to import",
  );
  await page.evaluate(() => (window as any).__drag("leave"));
  await expect(page.locator(".library-page")).toHaveAttribute(
    "data-dragging",
    "false",
  );
  await page.evaluate(() =>
    (window as any).__drag("drop", [
      "/test/One.ipa",
      "/test/notes.txt",
      "/test/Bad.ipa",
      "/test/Two.IPA",
    ]),
  );
  await expect(
    page.getByRole("list", { name: "Import results" }),
  ).toContainText("Only IPA files can be imported");
  await expect(
    page.getByRole("list", { name: "Import results" }),
  ).toContainText("Invalid IPA");
  await expect(page.locator(".library-row")).toContainText("2 saved versions");
  expect(
    await page.evaluate(() =>
      (window as any).__calls
        .filter((c: any) => c.cmd === "library_import")
        .map((c: any) => c.args.path),
    ),
  ).toEqual(["/test/One.ipa", "/test/Bad.ipa", "/test/Two.IPA"]);
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await page.evaluate(() =>
    (window as any).__drag("drop", ["/test/Hidden.ipa"]),
  );
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter((c: any) => c.cmd === "library_import")
          .length,
    ),
  ).toBe(3);
  await expect
    .poll(() => page.evaluate(() => (window as any).__hasDropListener()))
    .toBe(false);
  await page.getByRole("link", { name: "App Library", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => (window as any).__hasDropListener()))
    .toBe(true);
  await page.evaluate(() =>
    (window as any).__drag("drop", ["/test/Third.ipa"]),
  );
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toHaveCount(3);
});

test("cached app icons appear in rows and details with a fallback for unreadable images", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => {
    const png =
      "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    // The manifest holds a hash; the bytes are fetched once per hash and cached in the window.
    (window as any).__icons = { "icon-hash": png };
    (window as any).__library.apps[0].icon_sha = "icon-hash";
    (window as any).__workspaceIcon = png;
    window.dispatchEvent(new Event("library-changed"));
  });
  await expect(
    page.getByRole("img", { name: "Library App icon" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      page
        .getByRole("img", { name: "Library App icon" })
        .evaluate((img: HTMLImageElement) => img.naturalWidth),
    )
    .toBe(1);
  await page.getByRole("link", { name: "All apps" }).click();
  await expect(
    page.locator(".library-row").getByRole("img", { name: "Library App icon" }),
  ).toBeVisible();
  await page
    .getByRole("img", { name: "Library App icon" })
    .dispatchEvent("error");
  await expect(page.locator(".library-row .library-icon svg")).toHaveAttribute(
    "aria-label",
    "App icon unavailable",
  );
  await page.locator(".library-row").click();
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await expect(
    page.locator(".guided-card").getByRole("img", { name: "Library App icon" }),
  ).toBeVisible();
});

test("import result opens its saved version and removal icon stays aligned with its label", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => (window as any).__addSigned());
  for (const width of [1120, 780, 680]) {
    await page.setViewportSize({ width, height: 840 });
    for (const name of ["Remove version", "Remove signed build"]) {
      const action = page.getByRole("button", { name, exact: true });
      const icon = await action.locator("svg").boundingBox();
      const label = await action.locator("span").boundingBox();
      expect(icon && label).toBeTruthy();
      expect(
        Math.abs(icon!.y + icon!.height / 2 - label!.y - label!.height / 2),
      ).toBeLessThan(2);
      expect(label!.x - icon!.x - icon!.width).toBeGreaterThanOrEqual(7);
      expect(label!.height).toBeLessThan(23);
    }
    if (width > 760) {
      for (const row of await page.locator(".library-version").all()) {
        const bounds = await row.boundingBox();
        const primary = await row.locator(".library-actions a").boundingBox();
        expect(Math.abs(primary!.y - bounds!.y - 24)).toBeLessThan(2);
      }
    }
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  }
  await page
    .getByRole("link", { name: "Open imported version", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/ipas\/app-1\/workspace\/version-1$/);
  await expect(
    page.getByRole("heading", { name: "Install an app" }),
  ).toBeVisible();
});

/// Put a standing on screen. Every word of it is decided in Rust, so a test supplies the whole
/// record — including the sentence — and then asserts the page rendered exactly that.
async function counting(
  page: import("@playwright/test").Page,
  entries: Record<string, unknown>[],
) {
  await page.evaluate((values) => {
    const w = window as any;
    w.__library.expiries = values;
    localStorage.setItem("test-library", JSON.stringify(w.__library));
    window.dispatchEvent(new Event("library-changed"));
  }, entries);
}
const entry = (over: Record<string, unknown> = {}) => ({
  app_id: "app-1",
  artifact_id: "version-1",
  attempt_id: "attempt-1",
  device_id: "salted-device",
  device_name: "My iPhone",
  app_name: "Library App",
  identifier: "test.library",
  signed: true,
  expires: "2099-01-01T00:00:00Z",
  expires_unix: 4070908800,
  installed_unix: 200,
  standing: { state: "valid", days: 5 },
  bearing: "same_app",
  sentence:
    "Library App was installed from this team and stops launching in 5 days.",
  urgent: false,
  ...over,
});

test("an app that was never installed counts down from nothing", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  // A saved file is a fact about this Mac. Counting down from it would claim an installation.
  await expect(page.locator(".renewal")).toHaveCount(0);
  await page.getByRole("link", { name: "All apps" }).click();
  await expect(page.locator(".renewal")).toHaveCount(0);
  await expect(page.locator(".library-row")).toContainText(
    "No successful installation recorded",
  );
});

test("a live install counts down quietly and an expired one is announced", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await counting(page, [entry()]);
  // The page renders the sentence Rust built; it never assembles one from the parts.
  const quiet = page.locator(".renewal");
  await expect(quiet).toContainText("stops launching in 5 days");
  await expect(quiet).toContainText("My iPhone");
  await expect(quiet).not.toHaveClass(/renewal-urgent/);
  await expect(quiet).toHaveAttribute("data-standing", "valid");

  await counting(page, [
    entry({
      standing: { state: "expired", days: 2 },
      urgent: true,
      sentence:
        "Library App stopped launching 2 days ago. Re-sign and install it again.",
    }),
  ]);
  const loud = page.locator(".renewal");
  await expect(loud).toHaveClass(/renewal-urgent/);
  await expect(loud).toHaveAttribute("role", "status");
  await expect(loud).toContainText("stopped launching 2 days ago");
});

test("the copy still working leads the app row and the dead one stays visible", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await counting(page, [
    entry({ device_name: "Second phone" }),
    entry({
      attempt_id: "attempt-2",
      device_id: "device-two",
      device_name: "First phone",
      standing: { state: "expired", days: 2 },
      urgent: true,
      sentence:
        "Library App stopped launching 2 days ago. Re-sign and install it again.",
    }),
  ]);
  await page.getByRole("link", { name: "All apps" }).click();
  // A re-sign installed on one tester's phone does not revive the copy on another's, so the row
  // leads with the copy that still launches rather than raising a false alarm.
  await expect(page.locator(".library-row")).toContainText(
    "stops launching in 5 days",
  );
  await expect(page.locator(".library-row")).not.toContainText(
    "stopped launching 2 days ago",
  );
});

test("the workspace asks about the build on screen and says nothing about another", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await counting(page, [
    entry({
      standing: { state: "expired", days: 2 },
      urgent: true,
      sentence:
        "Library App stopped launching 2 days ago. Re-sign and install it again.",
    }),
  ]);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Install an app" }),
  ).toBeVisible();
  const banner = page.locator(".guided-context [data-standing]");
  await expect(banner).toHaveAttribute("data-standing", "expired");
  // Re-signing is blocked until an account and a team exist, so the banner must not invite it.
  await expect(page.getByRole("button", { name: "Re-sign now" })).toHaveCount(
    0,
  );
  await expect
    .poll(async () =>
      page.evaluate(() =>
        ((window as any).__calls as any[])
          .filter((c) => c.cmd === "library_expiry")
          .map((c) => c.args.artifactId),
      ),
    )
    .toContain("version-1");
});

test("a build signed for another team shows no countdown", async ({ page }) => {
  await mock(page);
  await imported(page);
  await counting(page, [
    entry({
      bearing: "other_team",
      standing: { state: "valid", days: 5 },
      urgent: false,
      sentence:
        "The last build Orbiter installed, Library App, was signed for a different team.",
    }),
  ]);
  const banner = page.locator(".renewal");
  await expect(banner).toHaveAttribute("data-bearing", "other_team");
  await expect(banner).toContainText("signed for a different team");
  await expect(banner).not.toContainText("5 days");
  await expect(banner).not.toHaveClass(/renewal-urgent/);
});

test("an expired version enters the guided signing flow without automatically signing", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await counting(page, [
    entry({
      standing: { state: "expired", days: 2 },
      urgent: true,
      sentence: "Library App expired two days ago.",
    }),
  ]);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await expect(page.locator(".guided-context")).toContainText(
    "expired two days ago",
  );
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Re-sign with my Apple account" })
    .click();
  await expect(page.getByLabel("Apple account email")).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter(
          (c: any) => c.cmd === "library_execute_preparation",
        ).length,
    ),
  ).toBe(0);
});

test("files nothing points at are named and only removed when asked", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.getByRole("link", { name: "All apps" }).click();
  await expect(page.getByRole("button", { name: /Reclaim/ })).toHaveCount(0);

  await page.evaluate(() => {
    const w = window as any;
    w.__library.unreferenced_bytes = 3 * 1024 * 1024;
    window.dispatchEvent(new Event("library-changed"));
  });
  // Bytes an interrupted copy left behind are counted and named, never quietly deleted.
  await expect(page.locator(".library-storage")).toContainText(
    "3.0 MB not referenced by any saved version",
  );
  await page
    .getByRole("button", { name: "Reclaim unreferenced files" })
    .click();
  await expect(page.locator(".library-storage")).not.toContainText(
    "not referenced",
  );
  await expect(page.getByRole("button", { name: /Reclaim/ })).toHaveCount(0);
});

test("a structured backend failure is shown as its message, not as an object", async ({
  page,
}) => {
  await mock(page);
  // Refactored commands reject with `{ code, message }` rather than a bare string. The window
  // must read the message; rendering "[object Object]" would lose the failure entirely.
  await page.evaluate(() => {
    (window as any).__corrupt = {
      code: "storage_corrupt",
      message: "Library storage is corrupt. Restore the manifest.",
    };
    window.dispatchEvent(new Event("library-changed"));
  });
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("Library storage is corrupt");
  await expect(alert).not.toContainText("object Object");
});

/// Seed a signed build that was installed and has now expired, which is the only state the refresh
/// action exists for. Written directly rather than driven through signing and installing: those
/// paths have their own tests, and what is under test here is what happens *after* the seven days.
async function expired(page: Page) {
  await page.evaluate(() => {
    const w = window as any;
    // A Watch app and a marker that is not the default, so a prefill can be told apart from a
    // control that simply started out that way.
    w.__watch = true;
    w.__addSigned();
    w.__library.artifacts.find((a: any) => a.id === "signed-1").marker = "beta";
    w.__library.devices = [
      { id: "salted-device", name: "My iPhone", last_seen_unix: 200 },
    ];
    w.__library.attempts = [
      {
        id: "attempt-1",
        app_id: "app-1",
        artifact_id: "signed-1",
        device_id: "salted-device",
        app_name: "Library App",
        identifier: "test.library.signed",
        version: "preview",
        build: "alpha",
        sha256: "b".repeat(64),
        signed: true,
        expires: "2099-01-01T00:00:00Z",
        started_unix: 200,
        finished_unix: 201,
        stage: "installed",
        message: "iOS reported completion.",
      },
    ];
    localStorage.setItem("test-library", JSON.stringify(w.__library));
  });
  await counting(page, [
    entry({
      artifact_id: "signed-1",
      standing: { state: "expired", days: 2 },
      urgent: true,
      sentence:
        "Library App stopped launching 2 days ago. Re-sign and install it again.",
    }),
  ]);
}

test("a dead build's countdown opens the original with the previous answers filled in", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await expired(page);

  await page
    .getByRole("button", { name: "Sign and install again", exact: true })
    .click();
  // The original, never the signed build: a spent profile cannot be signed again.
  await expect(page).toHaveURL(/#\/ipas\/app-1\/workspace\/version-1$/);
  const note = page.locator("[data-refresh='true']");
  await expect(note).toContainText("Signing Library App again");
  // Named, so a person with two testers is not sent to re-sign for the wrong phone.
  await expect(note).toContainText("My iPhone");
  // Filled in from what the expired build was signed under. The control lives further into the
  // flow, so reaching it is the only way to assert the prefill actually landed rather than
  // assuming the state behind it.
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Re-sign with my Apple account" })
    .click();
  await page.getByLabel("Apple account email").fill("local@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page.getByRole("checkbox", { name: /I agree to authenticate/ }).check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  // Both answers the expired build was signed under, neither of them a default.
  await expect(page.getByLabel("Included Watch app")).toHaveValue("remove");
  await page.getByText("Advanced signing options", { exact: true }).click();
  await expect(page.getByLabel("App name marker")).toHaveValue("beta");
  // Filled in, not decided: nothing was signed on the way here.
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter(
          (c: any) => c.cmd === "library_execute_preparation",
        ).length,
    ),
  ).toBe(0);
});

test("a refresh says when the phone on the cable is a different one, and nothing when it cannot tell", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await expired(page);
  await page.evaluate(() => {
    (window as any).__deviceTag = "a-different-phone";
  });

  await page
    .getByRole("button", { name: "Sign and install again", exact: true })
    .click();
  const note = page.locator("[data-refresh='true']");
  await expect(note).toContainText("not the one that build was installed to");
  await expect(note).toContainText(
    "does not replace anything on the other one",
  );

  // Not being able to identify the phone is not evidence that it is the wrong one. Saying so
  // would send a person looking for a problem that is not there.
  await page.evaluate(() => {
    const w = window as any;
    delete w.__deviceTag;
    w.__deviceTagError = "Selected iPhone disconnected. Select it again.";
  });
  await page.reload();
  await expect(page.locator("[data-refresh='true']")).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Install an app" }),
  ).toBeVisible();
});

test("a countdown whose original is gone still counts down and offers nothing", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await expired(page);
  await expect(
    page.getByRole("button", { name: "Sign and install again", exact: true }),
  ).toBeVisible();

  // The saved file is removed; the history, and the fact that a tester's app has stopped
  // working, both survive it. What does not survive is the ability to do anything about it.
  await page.evaluate(() => {
    const w = window as any;
    w.__library.artifacts.forEach((a: any) => {
      if (a.id === "version-1") a.deleted = true;
    });
    localStorage.setItem("test-library", JSON.stringify(w.__library));
    window.dispatchEvent(new Event("library-changed"));
  });
  await page.reload();
  await expect(page.locator(".renewal")).toContainText(
    "stopped launching 2 days ago",
  );
  await expect(
    page.getByRole("button", { name: "Sign and install again", exact: true }),
  ).toHaveCount(0);
});
