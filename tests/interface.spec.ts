import { test, expect } from "@playwright/test";
const report = {
  size_bytes: 1024 * 1024,
  main_path: "Payload/Test.app",
  icon_data_url: null,
  bundles: [
    {
      path: "Payload/Test.app",
      kind: "Main app",
      name: "Synthetic Test App",
      identifier: "test.synthetic",
      version: "1.0",
      build: "1",
      minimum_os: "15.0",
      supported_platforms: ["iPhoneOS"],
      device_families: [1],
      slices: [
        {
          architecture: "arm64",
          encrypted: false,
          entitlements: {},
          xml_entitlements_present: false,
          der_entitlements_present: false,
        },
      ],
      profile: null,
      issues: [],
    },
  ],
  findings: [
    {
      status: "not_verified",
      title: "Signing identity required",
      detail: "No target team selected.",
      bundle: null,
    },
  ],
};
test("browser preview keeps native and signing actions unavailable", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByText("Browser preview.", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Sign & Install" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: /Drop your IPA/ }),
  ).toBeDisabled();
  await page.getByText("Advanced options", { exact: true }).click();
  await expect(
    page.getByText("Your app identifiers and capabilities are left intact."),
  ).toBeVisible();
  await page.screenshot({ path: "test-results/preview.png", fullPage: true });
});
async function nativeMock(
  page: import("@playwright/test").Page,
  mode: "success" | "error" | "cancel",
) {
  await page.addInitScript(
    ({ report, mode }) => {
      let pendingReject: ((e: string) => void) | undefined;
      let callbackId = 0;
      (window as any).isTauri = true;
      const callbacks = new Map();
      (window as any).__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main" },
        },
        transformCallback: (fn: unknown) => {
          callbacks.set(++callbackId, fn);
          return callbackId;
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        invoke: async (cmd: string, args?: any) => {
          if (cmd === "local_auth_support")
            return (
              (window as any).__supportResult ?? {
                available: true,
                message: "Local macOS authentication support is available.",
              }
            );
          if (cmd === "account_status")
            return (
              (window as any).__accountView ?? {
                stage: "signed_out",
                account: null,
                teams: [],
                selected_team: null,
                challenge: null,
                message: "",
              }
            );
          if (cmd === "account_sign_in") {
            (window as any).__loginRequested = {
              consent: args.consent,
              email: args.email,
            };
            return ((window as any).__accountView = {
              stage: "two_factor",
              account: null,
              teams: [],
              selected_team: null,
              message: "Enter your verification code.",
              challenge: {
                id: "challenge-1",
                sms: false,
                unknown: false,
                retry: false,
                numbers: [{ id: 1, label: "Phone ending in 12" }],
              },
            });
          }
          if (cmd === "account_answer") {
            (window as any).__answerRequested = {
              challengeId: args.challengeId,
              action: args.answer.action,
            };
            return ((window as any).__accountView = {
              stage: "signed_in",
              account: "test@example.invalid",
              selected_team: null,
              challenge: null,
              message: "Signed in. Select a team.",
              teams: [
                {
                  id: "TEAM1",
                  name: "Synthetic Team",
                  kind: "Company",
                  free: false,
                  membership: "Apple Developer Program",
                },
                {
                  id: "TEAM2",
                  name: "Synthetic Personal",
                  kind: "Individual",
                  free: true,
                  membership: null,
                },
              ],
            });
          }
          if (cmd === "account_select_team") {
            (window as any).__accountView.selected_team = args.id;
            return (window as any).__accountView;
          }
          if (cmd === "account_sign_out")
            return ((window as any).__accountView = {
              stage: "signed_out",
              account: null,
              teams: [],
              selected_team: null,
              challenge: null,
              message: "Signed out locally.",
            });
          if (cmd === "account_register_device") {
            (window as any).__registerRequested = {
              deviceId: args.deviceId,
              acknowledged: args.acknowledged,
            };
            return {
              registration: "registered",
              team_devices: 1,
              message: "This iPhone is now registered on the selected team.",
            };
          }
          if (cmd === "account_request_certificate") {
            (window as any).__certificateRequested = {
              acknowledged: args.acknowledged,
            };
            return {
              reused: false,
              expires: "2026-09-19T00:00:00Z",
              active: 1,
              message: "A development certificate was issued.",
            };
          }
          if (cmd === "account_refresh_teams")
            return ((window as any).__accountView = {
              stage: "signed_out",
              account: null,
              teams: [],
              selected_team: null,
              challenge: null,
              message:
                "Developer session could not be refreshed. Sign in again; your team selection was cleared.",
            });
          if (cmd === "discover_signing_identities") {
            if ((window as any).__identityError) throw "Keychain failed";
            return (window as any).__identityResult;
          }
          if (cmd === "installation_status")
            return (window as any).__jobResult ?? null;
          if (cmd === "prepare_install") return (window as any).__reviewResult;
          if (cmd === "discard_install") return;
          if (cmd === "execute_install") {
            (window as any).__executed = args;
            return (window as any).__jobResult;
          }
          if (cmd === "discover_devices")
            return (
              (window as any).__deviceResult ?? {
                devices: [],
                service_available: true,
                message: null,
              }
            );
          if (cmd === "plugin:event|listen") return 1;
          if (cmd === "plugin:dialog|open") return "/synthetic/Test.ipa";
          if (cmd === "inspect_ipa") {
            if (mode === "error")
              throw "Invalid or unsupported ZIP archive. Export a fresh IPA from your build system.";
            if (mode === "cancel")
              return await new Promise((_resolve, reject) => {
                pendingReject = reject;
              });
            return report;
          }
          if (cmd === "cancel_inspection") {
            pendingReject?.(
              "Inspection cancelled. The original IPA is unchanged.",
            );
          }
        },
      };
      (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: () => {},
      };
    },
    { report, mode },
  );
  await page.goto("/");
}
test("selected file renders inspection and retains unavailable signing", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
  await expect(
    page.getByRole("heading", { name: "Synthetic Test App" }),
  ).toBeVisible();
  await expect(
    page.getByText("Inspection complete", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Sign & Install" }),
  ).toBeDisabled();
  await expect(page.getByLabel("Signing team")).toBeDisabled();
  await page.getByText("Bundle inspection details", { exact: false }).click();
  await page.getByText("Main app", { exact: true }).click();
  await expect(
    page.getByText("No XML or DER entitlements found"),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/inspection.png",
    fullPage: true,
  });
});
test("failed inspection gives a recovery instruction and permits retry", async ({
  page,
}) => {
  await nativeMock(page, "error");
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
  await expect(page.getByRole("alert")).toContainText("Export a fresh IPA");
  await expect(
    page.getByRole("button", { name: /Drop your IPA/ }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Sign & Install" }),
  ).toBeDisabled();
});
test("cancel waits for worker acknowledgement and permits another inspection", async ({
  page,
}) => {
  await nativeMock(page, "cancel");
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
  await page.getByRole("button", { name: "Cancel inspection" }).click();
  await expect(page.getByRole("alert")).toContainText("Inspection cancelled");
  await expect(
    page.getByRole("button", { name: /Drop your IPA/ }),
  ).toBeEnabled();
});

test("device refresh removes disconnected selection and surfaces service errors", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.evaluate(() => {
    (window as any).__deviceResult = {
      devices: [
        {
          id: 1,
          name: "Synthetic iPhone",
          product_type: "iPhoneTest",
          ios_version: "18.0",
          connection: "USB",
          state: "paired",
          message: "Synthetic pairing verified.",
        },
      ],
      service_available: true,
      message: null,
    };
  });
  await page.getByRole("button", { name: "Refresh devices" }).click();
  await expect(page.getByLabel("Physical iPhone")).toHaveValue("1");
  await expect(page.getByText("Synthetic pairing verified.")).toBeVisible();
  await page.evaluate(() => {
    (window as any).__deviceResult = {
      devices: [],
      service_available: true,
      message: null,
    };
  });
  await page.getByRole("button", { name: "Refresh devices" }).click();
  await expect(page.getByLabel("Physical iPhone")).toHaveValue("");
  await expect(page.getByLabel("Physical iPhone")).toBeDisabled();
  await expect(page.getByText("Synthetic pairing verified.")).toHaveCount(0);
  await page.evaluate(() => {
    (window as any).__deviceResult = {
      devices: [],
      service_available: false,
      message: "Synthetic service unavailable.",
    };
  });
  await page.getByRole("button", { name: "Refresh devices" }).click();
  await expect(page.getByText("Synthetic service unavailable.")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Sign & Install" }),
  ).toBeDisabled();
});

async function readyForReview(
  page: import("@playwright/test").Page,
  blockers: string[] = [],
) {
  await nativeMock(page, "success");
  await page.evaluate((blockers) => {
    (window as any).__deviceResult = {
      devices: [
        {
          id: 1,
          name: "Synthetic iPhone",
          product_type: "iPhoneTest",
          ios_version: "18.0",
          connection: "USB",
          state: "paired",
          message: "Synthetic pairing verified.",
        },
      ],
      service_available: true,
      message: null,
    };
    (window as any).__reviewResult = {
      token: "synthetic-review",
      app_name: "Synthetic Test App",
      bundle_id: "test.synthetic",
      version: "1.0",
      device_name: "Synthetic iPhone",
      size_bytes: 1048576,
      sha256: "synthetic-fingerprint",
      existing_app: { version: "0.9", build: "1" },
      blockers,
      notes: ["iOS validates the existing signature."],
    };
  }, blockers);
  await page.getByRole("button", { name: "Refresh devices" }).click();
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
  await page.getByRole("button", { name: "Review installation" }).click();
}
test("installation needs explicit acknowledgement and preserves unknown outcomes", async ({
  page,
}) => {
  await readyForReview(page);
  await expect(
    page.getByText("This installation may replace it.", { exact: false }),
  ).toBeVisible();
  const install = page.getByRole("button", { name: "Install unchanged IPA" });
  await expect(install).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__executed)).toBeUndefined();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.evaluate(() => {
    (window as any).__jobResult = {
      id: "synthetic-review",
      stage: "unknown",
      message:
        "Connection lost after the install command. Check the phone before retrying.",
      transferred_bytes: 1048576,
      total_bytes: 1048576,
      device_percent: null,
      cleanup_pending: true,
    };
  });
  await install.click();
  await expect(
    page.getByText("Installation outcome unknown", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("iOS reported installation complete", { exact: true }),
  ).toHaveCount(0);
  expect(
    (await page.evaluate(() => (window as any).__executed)).acknowledged,
  ).toBe(true);
  await page.screenshot({
    path: "test-results/install-outcome.png",
    fullPage: true,
  });
});
test("profile blockers prevent the installation action", async ({ page }) => {
  await readyForReview(page, [
    "The embedded profile does not authorize this iPhone.",
  ]);
  await expect(page.getByRole("alert")).toContainText(
    "does not authorize this iPhone",
  );
  await expect(
    page.getByRole("button", { name: "Install unchanged IPA" }),
  ).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__executed)).toBeUndefined();
});

test("a reopened window follows an active installation until its terminal outcome", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__jobResult = {
      id: "active-job",
      stage: "transferring",
      message: "Synthetic active transfer.",
      transferred_bytes: 10,
      total_bytes: 100,
      device_percent: null,
      cleanup_pending: true,
    };
  });
  await nativeMock(page, "success");
  await expect(page.getByText("Synthetic active transfer.")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Cancel transfer" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /Drop your IPA/ }),
  ).toBeDisabled();
  await page.evaluate(() => {
    (window as any).__jobResult = {
      id: "active-job",
      stage: "unknown",
      message: "Synthetic connection loss after dispatch.",
      transferred_bytes: 100,
      total_bytes: 100,
      device_percent: 100,
      cleanup_pending: true,
    };
  });
  await expect(
    page.getByText("Installation outcome unknown", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Cancel transfer" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: /Drop your IPA/ }),
  ).toBeEnabled();
});

test("local identity inventory clears stale certificates on failed refresh", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.evaluate(() => {
    (window as any).__identityResult = {
      identities: [
        { fingerprint: "A".repeat(40), name: "Apple Development: Synthetic" },
      ],
      available: true,
      message: "Profile matching has not been tested.",
    };
  });
  await page.getByRole("button", { name: "Check Keychain" }).click();
  await expect(
    page.getByText("Apple Development: Synthetic", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Sign & Install" }),
  ).toBeDisabled();
  await page.evaluate(() => {
    (window as any).__identityError = true;
  });
  await page.getByRole("button", { name: "Check Keychain" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Could not check local signing identities",
  );
  await expect(
    page.getByText("Apple Development: Synthetic", { exact: true }),
  ).toHaveCount(0);
});

test("account flow works without a manual support check, requires consent, and never auto-selects a team", async ({
  page,
}) => {
  await nativeMock(page, "success");

  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await expect(
    page.getByRole("button", { name: "Sign in to Apple" }),
  ).toBeDisabled();
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await expect(page.getByLabel("Password", { exact: true })).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Verify code" }),
  ).toBeDisabled();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  await expect(page.getByLabel("Signing team")).toHaveValue("");
  await page.getByLabel("Signing team").selectOption("TEAM2");
  await expect(page.getByLabel("Signing team")).toHaveValue("TEAM2");
  await expect(
    page.getByRole("button", { name: "Sign & Install" }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  // Signing out clears the secrets, not the account address or the consent already given, so
  // signing back in needs only the password. The sign-in button must not be stuck disabled.
  await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
  await expect(page.getByLabel("Apple account email")).toHaveValue(
    "test@example.invalid",
  );
  await expect(
    page.getByLabel("I agree to authenticate directly with Apple", {
      exact: false,
    }),
  ).toBeChecked();
  await expect(page.getByLabel("Signing team")).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Sign in to Apple" }),
  ).toBeDisabled();
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await expect(
    page.getByRole("button", { name: "Sign in to Apple" }),
  ).toBeEnabled();
});

test("registering an iPhone needs a device, a team, and an explicit acknowledgement", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.evaluate(() => {
    (window as any).__deviceResult = {
      devices: [
        {
          id: 1,
          name: "Synthetic iPhone",
          product_type: "iPhoneTest",
          ios_version: "18.0",
          connection: "USB",
          state: "paired",
          message: "Synthetic pairing verified.",
        },
      ],
      service_available: true,
      message: null,
    };
  });
  await page.getByRole("button", { name: "Refresh devices" }).click();
  await expect(page.getByLabel("Physical iPhone")).toHaveValue("1");
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  // No team chosen yet: registration is not even offered.
  await expect(
    page.getByRole("button", { name: "Register iPhone on team" }),
  ).toHaveCount(0);
  await page.getByLabel("Signing team").selectOption("TEAM2");
  const register = page.getByRole("button", {
    name: "Register iPhone on team",
  });
  await expect(register).toBeDisabled();
  await page
    .getByLabel("I understand a free personal team", { exact: false })
    .check();
  await expect(register).toBeEnabled();
  await register.click();
  await expect(
    page.getByText("now registered on the selected team", { exact: false }),
  ).toBeVisible();
  // The acknowledgement actually reached the backend with the selected device.
  expect(
    await page.evaluate(() => (window as any).__registerRequested),
  ).toEqual({ deviceId: 1, acknowledged: true });
  // The certificate is a separate acknowledgement: registering does not imply it.
  const certificate = page.getByRole("button", {
    name: "Get signing certificate",
  });
  await expect(certificate).toBeDisabled();
  await page
    .getByLabel("I understand this uses one of the team's", { exact: false })
    .check();
  await certificate.click();
  await expect(
    page.getByText("development certificate was issued", { exact: false }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => (window as any).__certificateRequested),
  ).toEqual({ acknowledged: true });

  // Changing the team invalidates the result rather than carrying it across.
  await page.getByLabel("Signing team").selectOption("TEAM1");
  await expect(
    page.getByText("now registered on the selected team", { exact: false }),
  ).toHaveCount(0);
  await expect(
    page.getByText("development certificate was issued", { exact: false }),
  ).toHaveCount(0);
});

test("cancelled verification clears secrets and failed team refresh clears selection", async ({
  page,
}) => {
  await nativeMock(page, "success");
  async function signIn() {
    await page
      .getByRole("button", { name: "Check local support", exact: true })
      .click();
    await page.getByLabel("Apple account email").fill("test@example.invalid");
    await page
      .getByLabel("Password", { exact: true })
      .fill("synthetic-password");
    await page
      .getByLabel("I agree to authenticate directly with Apple", {
        exact: false,
      })
      .check();
    await page.getByRole("button", { name: "Sign in to Apple" }).click();
  }
  await signIn();
  await page.getByLabel("Verification code").fill("123");
  await page.getByRole("button", { name: "Cancel sign-in" }).click();
  await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
  await signIn();
  await page.getByRole("button", { name: "Send SMS", exact: false }).click();
  await page.getByLabel("Signing team").selectOption("TEAM1");
  await page.getByRole("button", { name: "Refresh teams" }).click();
  await expect(page.getByLabel("Signing team")).toBeDisabled();
  await expect(
    page.getByText("Developer session could not be refreshed.", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
});

test("unavailable local support prevents sign-in without a remote fallback", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.evaluate(() => {
    (window as any).__supportResult = {
      available: false,
      message:
        "Local authentication is unavailable. No remote fallback was used.",
    };
  });
  await page
    .getByRole("button", { name: "Check local support", exact: true })
    .click();
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await expect(
    page.getByRole("button", { name: "Sign in to Apple" }),
  ).toBeDisabled();
  await expect(
    page.getByText("Local authentication is unavailable.", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByText("ani.stikstore.app", { exact: false }),
  ).toHaveCount(0);
});
