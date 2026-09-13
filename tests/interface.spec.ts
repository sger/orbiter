import { test, expect, type Page } from "@playwright/test";
import { mock, imported } from "./helpers/library";

async function choose(page: Page, readiness = "direct") {
  await mock(page);
  await imported(page);
  await page.evaluate((value) => {
    (window as any).__readiness = value;
  }, readiness);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Check app & iPhone" }),
  ).toBeEnabled();
}
async function account(page: Page) {
  await choose(page, "needs_signing");
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Continue with Apple account" })
    .click();
}
async function login(page: Page) {
  await page.getByLabel("Apple account email").fill("local@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page.getByRole("checkbox", { name: /I agree to authenticate/ }).check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
}
async function consent(page: Page) {
  for (const label of [
    /I approve registering/,
    /I approve obtaining/,
    /I approve reserving|I approve signing/,
  ]) {
    const checkbox = page.getByRole("checkbox", { name: label });
    if (await checkbox.count()) await checkbox.check();
  }
}

test("browser preview cannot import or contact an iPhone", async ({ page }) => {
  await page.goto("/#/ipas/workspace");
  await expect(
    page.getByRole("button", { name: /Drop IPA files here/ }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Check app & iPhone" }),
  ).toBeDisabled();
  await expect(page.getByLabel("Apple account email")).not.toBeVisible();
});

test("direct installation skips Apple and requires final artifact acknowledgement", async ({
  page,
}) => {
  await choose(page);
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await expect(
    page.getByRole("heading", { name: "Ready for installation review" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Install on iPhone" }),
  ).toBeDisabled();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install on iPhone" }).click();
  await expect(
    page.getByRole("heading", { name: "Installation complete", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter((c: any) =>
          ["account_sign_in", "library_execute_preparation"].includes(c.cmd),
        ).length,
    ),
  ).toBe(0);
});

test("non-signable blockers do not offer Apple signing", async ({ page }) => {
  await choose(page, "blocked");
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await expect(
    page.getByText("Resolve these issues before continuing."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Continue with Apple account" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Review installation", exact: true }),
  ).toHaveCount(0);
});

for (const paid of [false, true]) {
  test(`${paid ? "Developer Program" : "Personal Team"} signs only after reviewing required actions`, async ({
    page,
  }) => {
    await account(page);
    if (paid)
      await page.evaluate(() => {
        (window as any).__teams = [
          {
            id: "PAID",
            name: "Personal Projects",
            free: false,
            kind: "Individual",
            membership: "Apple Developer Program",
          },
        ];
      });
    await page.getByLabel("Apple account email").fill("local@example.invalid");
    await page
      .getByLabel("Password", { exact: true })
      .fill("synthetic-password");
    await expect(
      page.getByRole("button", { name: "Sign in to Apple" }),
    ).toBeDisabled();
    await login(page);
    await expect(
      page.getByRole("button", { name: "Review preparation" }),
    ).toBeEnabled();
    await page.getByRole("button", { name: "Review preparation" }).click();
    await expect(
      page.getByRole("button", { name: "Prepare & sign", exact: true }),
    ).toBeDisabled();
    await consent(page);
    await page
      .getByRole("button", { name: "Prepare & sign", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Review installation", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Install on iPhone" }),
    ).toBeDisabled();
    expect(
      await page.evaluate(
        () =>
          (window as any).__calls.filter(
            (c: any) => c.cmd === "execute_install",
          ).length,
      ),
    ).toBe(0);
    expect(
      await page.evaluate(() => (window as any).__signRequested.consents),
    ).toEqual({ registration: true, certificate: true, provisioning: true });
    const lastCheck = await page.evaluate(
      () =>
        (window as any).__calls
          .filter((c: any) => c.cmd === "library_prepare_install")
          .at(-1).args,
    );
    expect(lastCheck.artifactId).toBe("signed-1");
  });
}

test("multiple teams require a choice and unknown membership cannot prepare", async ({
  page,
}) => {
  await account(page);
  await page.evaluate(() => {
    (window as any).__teams = [
      { id: "ONE", name: "Personal", free: true },
      { id: "UNKNOWN", name: "Unverified", free: null },
    ];
  });
  await login(page);
  await expect(
    page.getByRole("button", { name: "Review preparation" }),
  ).toBeDisabled();
  await page
    .getByLabel("Signing team", { exact: true })
    .selectOption("UNKNOWN");
  await expect(
    page.getByText("This team’s membership is unknown or unsupported.", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Review preparation" }),
  ).toBeDisabled();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter(
          (c: any) => c.cmd === "library_review_preparation",
        ).length,
    ),
  ).toBe(0);
});

test("Apple verification is conditional and secrets are cleared", async ({
  page,
}) => {
  await account(page);
  await page.evaluate(() => {
    (window as any).__challenge = {
      id: "challenge",
      retry: false,
      unknown: false,
      sms: false,
      numbers: [{ id: 1, label: "••42" }],
    };
  });
  await login(page);
  await expect(page.getByLabel("Verification code")).toBeVisible();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  await expect(
    page.getByRole("button", { name: "Review preparation" }),
  ).toBeEnabled();
  expect(await page.locator("#apple-password").count()).toBe(0);
});

test("verified resources are reused without new-resource consent", async ({
  page,
}) => {
  await account(page);
  await login(page);
  await page.evaluate(() => {
    (window as any).__reuse = true;
  });
  await page.getByRole("button", { name: "Review preparation" }).click();
  await expect(
    page.getByText("Reuse the existing signing key and certificate"),
  ).toBeVisible();
  await expect(
    page.getByRole("checkbox", { name: /I approve registering/ }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("checkbox", { name: /I approve obtaining/ }),
  ).toHaveCount(0);
  await consent(page);
  await page
    .getByRole("button", { name: "Prepare & sign", exact: true })
    .click();
  expect(
    await page.evaluate(() => (window as any).__signRequested.consents),
  ).toEqual({ registration: false, certificate: false, provisioning: true });
});

test("changing options discards a preparation and its consent", async ({
  page,
}) => {
  await account(page);
  await login(page);
  await page.getByRole("button", { name: "Review preparation" }).click();
  await consent(page);
  await page.getByRole("button", { name: "Change signing setup" }).click();
  await page.getByText("Advanced signing options", { exact: true }).click();
  await page.getByLabel("App name marker").fill("preview");
  await page.getByRole("button", { name: "Review preparation" }).click();
  await expect(
    page.getByRole("button", { name: "Prepare & sign", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() =>
      (window as any).__calls.some(
        (c: any) => c.cmd === "library_discard_preparation",
      ),
    ),
  ).toBe(true);
});

test("preparation survives navigation and disables account changes", async ({
  page,
}) => {
  await account(page);
  await login(page);
  await page.evaluate(() => {
    (window as any).__holdSign = true;
  });
  await page.getByRole("button", { name: "Review preparation" }).click();
  await consent(page);
  await page
    .getByRole("button", { name: "Prepare & sign", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Sign out", exact: true }),
  ).toBeDisabled();
  await page.getByRole("link", { name: "App Library", exact: true }).click();
  await page.getByRole("link", { name: /Signing in progress/ }).click();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter(
          (c: any) => c.cmd === "library_execute_preparation",
        ).length,
    ),
  ).toBe(1);
  await page.evaluate(() => (window as any).__finishSign());
  await expect(
    page.getByRole("button", { name: "Review installation", exact: true }),
  ).toBeVisible();
});

test("certificate conflict recovery is explicit and never follows unrelated failures", async ({
  page,
}) => {
  await account(page);
  await login(page);
  await page.evaluate(() => {
    (window as any).__prepareError = {
      code: "certificate_conflict",
      message: "No certificate slots available.",
    };
  });
  await page.getByRole("button", { name: "Review preparation" }).click();
  await consent(page);
  await page
    .getByRole("button", { name: "Prepare & sign", exact: true })
    .click();
  await page
    .getByText("Certificate conflict recovery", { exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Withdraw development certificates" }),
  ).toBeDisabled();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter(
          (c: any) => c.cmd === "account_withdraw_certificates",
        ).length,
    ),
  ).toBe(0);
});

test("unknown installation results never become success or automatically retry", async ({
  page,
}) => {
  await choose(page);
  await page.evaluate(() => {
    (window as any).__outcome = "unknown";
  });
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install on iPhone" }).click();
  await expect(
    page.getByText("Installation outcome unknown", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Installation complete", exact: true }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter((c: any) => c.cmd === "execute_install")
          .length,
    ),
  ).toBe(1);
});

test("guided drag and drop imports sequentially without signing or installing", async ({
  page,
}) => {
  await mock(page);
  await page
    .getByRole("button", { name: "Install an app", exact: true })
    .click();
  await expect(
    page.getByRole("heading", {
      name: "Choose an app and iPhone",
      exact: true,
    }),
  ).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => (window as any).__hasDropListener()))
    .toBe(true);
  await page.evaluate(() =>
    (window as any).__drag("drop", [
      "/test/One.ipa",
      "/test/Bad.ipa",
      "/test/Two.IPA",
    ]),
  );
  await expect(page.locator(".guided-context")).toContainText("Library App");
  await expect(
    page.getByText("Imported 2 of 3 selected files.", { exact: false }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter((c: any) =>
          ["execute_install", "library_execute_preparation"].includes(c.cmd),
        ).length,
    ),
  ).toBe(0);
  expect(
    await page.evaluate(() =>
      (window as any).__calls
        .filter((c: any) => c.cmd === "library_import")
        .map((c: any) => c.args.path),
    ),
  ).toEqual(["/test/One.ipa", "/test/Bad.ipa", "/test/Two.IPA"]);
});

for (const theme of ["light", "dark"] as const) {
  test(`${theme} guided screens fit minimum desktop dimensions and support keyboard navigation`, async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await page.setViewportSize({ width: 780, height: 640 });
    await choose(page);
    await expect(
      page.getByRole("heading", { name: "Install an app", exact: true }),
    ).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(
      page.getByRole("button", { name: /Import another IPA/ }),
    ).toBeFocused();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    const context = await page.locator(".guided-context").boundingBox();
    const card = await page.locator(".guided-card").boundingBox();
    expect(context!.y).toBeLessThan(card!.y);
    const heading = await page.locator(".guided-heading h2").boundingBox();
    const deviceLabel = await page.locator('label[for="device"]').boundingBox();
    expect(Math.abs(heading!.x - deviceLabel!.x)).toBeLessThan(2);
    await page.screenshot({
      path: `test-results/guided-${theme}-compact.png`,
      fullPage: true,
    });
  });
}

test("navigation keeps Help and Settings available and unknown routes recover", async ({
  page,
}) => {
  await page.goto("/#/ipas/workspace");
  await page.getByRole("link", { name: "Help", exact: true }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Help" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Settings" }),
  ).toBeVisible();
  await page.goto("/#/unrecognized");
  await expect(page).toHaveURL(/#\/ipas$/);
});

test("a locked iPhone cannot advance and provides focused help", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => {
    (window as any).__devices = [
      {
        id: 1,
        name: "Locked phone",
        connection: "USB",
        state: "locked",
        message: "Unlock the iPhone and review trust in Finder.",
      },
    ];
  });
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await page.getByRole("button", { name: "Refresh devices" }).click();
  await expect(
    page.getByText("Unlock the iPhone and review trust in Finder."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Check app & iPhone" }),
  ).toBeDisabled();
});

test("an included Watch app needs an explicit decision before preparation", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => {
    (window as any).__watch = true;
    (window as any).__readiness = "needs_signing";
  });
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Continue with Apple account" })
    .click();
  await login(page);
  await expect(
    page.getByRole("button", { name: "Review preparation" }),
  ).toBeDisabled();
  await page
    .getByLabel("Included Watch app", { exact: true })
    .selectOption("remove");
  await page.getByRole("button", { name: "Review preparation" }).click();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.find(
          (c: any) => c.cmd === "library_review_preparation",
        ).args.watch,
    ),
  ).toBe("remove");
});

test("stale installation review reports the failure and requires a new review", async ({
  page,
}) => {
  await choose(page);
  await page.evaluate(() => {
    (window as any).__installError = {
      code: "review_stale",
      message: "Review expired. Check the app and iPhone again.",
    };
  });
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install on iPhone" }).click();
  await expect(
    page.getByText("Review expired. Check the app and iPhone again."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Back to app and iPhone" }),
  ).toBeEnabled();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter((c: any) => c.cmd === "execute_install")
          .length,
    ),
  ).toBe(1);
});

test("reopening an active installation follows its terminal status without another execution", async ({
  page,
}) => {
  await choose(page);
  await page.evaluate(() => {
    localStorage.setItem("resume-test", "yes");
  });
  await page.addInitScript(() => {
    if (localStorage.getItem("resume-test"))
      (window as any).__resume = {
        id: "prior-operation",
        stage: "installing",
        message: "iOS is still installing the previously reviewed app.",
        transferred_bytes: 1000,
        total_bytes: 1000,
        device_percent: 50,
        cleanup_pending: false,
      };
  });
  await page.reload();
  await expect(
    page.getByRole("heading", {
      name: "Installing on your iPhone",
      exact: true,
    }),
  ).toBeVisible();
  await page.evaluate(() => {
    (window as any).__resume = {
      ...(window as any).__resume,
      stage: "unknown",
      message: "The connection ended before iOS confirmed the result.",
    };
  });
  await expect(
    page.getByText("Installation outcome unknown", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter((c: any) => c.cmd === "execute_install")
          .length,
    ),
  ).toBe(0);
});

test("a retained build opens its original before signing and reports a missing source", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.evaluate(() => (window as any).__addSigned());
  await page
    .getByRole("link", { name: "Review installation", exact: true })
    .click();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await page.evaluate(() => {
    (window as any).__library.artifacts.find(
      (a: any) => a.id === "version-1",
    ).deleted = true;
  });
  await page
    .getByRole("button", { name: "Open original to sign again" })
    .click();
  await expect(
    page.getByText("The original source is unavailable.", { exact: false }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__calls.filter(
          (c: any) => c.cmd === "library_execute_preparation",
        ).length,
    ),
  ).toBe(0);
});

test("App Library is the single app destination and its install action opens a fresh chooser", async ({
  page,
}) => {
  await choose(page);
  const navigation = page.getByRole("navigation", {
    name: "App navigation",
    exact: true,
  });
  await expect(navigation.getByRole("link")).toHaveCount(1);
  await expect(
    navigation.getByRole("link", { name: "App Library" }),
  ).toHaveAttribute("aria-current", "page");
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await expect(
    page.getByText("This IPA passed the local installation checks.", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: "Re-sign with my Apple account",
      exact: true,
    }),
  ).toBeVisible();
  await navigation.getByRole("link", { name: "App Library" }).click();
  await page
    .getByRole("button", { name: "Install an app", exact: true })
    .click();
  await expect(
    page.getByRole("heading", {
      name: "Choose an app and iPhone",
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.locator(".guided-context")).toContainText(
    "No app selected",
  );
  await expect(
    page.getByRole("button", { name: "Check app & iPhone" }),
  ).toBeDisabled();
});
