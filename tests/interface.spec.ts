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
    page.getByRole("button", { name: "Re-sign IPA" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: /Drop your IPA/ }),
  ).toBeDisabled();
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
            if ((window as any).__accountSignInResult)
              return ((window as any).__accountView = (
                window as any
              ).__accountSignInResult);
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
          if (cmd === "account_withdraw_certificates") {
            (window as any).__withdrawRequested = {
              acknowledged: args.acknowledged,
            };
            return "1 certificate(s) were withdrawn.";
          }
          if (cmd === "account_request_certificate") {
            (window as any).__certificateRequested = {
              acknowledged: args.acknowledged,
            };
            if ((window as any).__certificateFailure)
              throw (window as any).__certificateFailure;
            return {
              reused: false,
              expires: "2026-09-19T00:00:00Z",
              active: 1,
              message: "A development certificate was issued.",
            };
          }
          if (cmd === "start_device_log") {
            (window as any).__logRequested = {
              deviceId: args.deviceId,
              subjects: args.subjects,
            };
            // The mock receives the Channel itself, so deliver straight to its handler.
            args.progress?.onmessage?.({
              text: "Stoiximan[431]: social feed request refused",
            });
            return {
              matched: 1,
              discarded: 812,
              message: "Capture stopped. Nothing was written to disk.",
            };
          }
          if (cmd === "stop_device_log") return;
          if (cmd === "account_sign_ipa") {
            (window as any).__signRequested = {
              path: args.path,
              watch: args.watch,
            };
            if ((window as any).__signFailure)
              throw (window as any).__signFailure;
            // Real runs report counted progress before they return.
            args.progress?.onmessage?.({
              stage: "Signing bundles",
              done: 3,
              total: 24,
            });
            return {
              path: "/synthetic/signed/Test-TEAM2.ipa",
              identifier: "com.example.app.abc123",
              expires: "2026-09-19T00:00:00Z",
              bundles_signed: 3,
              removed: ["Payload/Test.app/Watch/Watch.app"],
              message:
                "A signed IPA was produced. The original IPA is unchanged.",
              log: [
                "Extracting the archive",
                "  4213 file(s), 207 MB",
                "Signing bundles",
                "  signed main app as com.example.app.abc123",
              ],
            };
          }
          if (cmd === "account_prepare_provisioning") {
            (window as any).__provisioningRequested = {
              path: args.path,
              acknowledged: args.acknowledged,
              watch: args.watch,
            };
            return {
              plan: {
                new_main_identifier: "com.example.app.abc123",
                blockers: [],
                consequences: [
                  "A Personal Team profile expires after seven days, so the app must be re-signed and reinstalled every week.",
                ],
                app_ids_required: 1,
                bundles: [
                  {
                    name: "App",
                    identifier: "com.example.app",
                    new_identifier: "com.example.app.abc123",
                    capabilities: [
                      {
                        key: "aps-environment",
                        action: "remove",
                        reason:
                          "A Personal Team cannot create a push capability.",
                        consequence:
                          "Push notifications stop working in the re-signed app.",
                      },
                    ],
                  },
                ],
              },
              app_ids: [
                {
                  identifier: "com.example.app.abc123",
                  created: true,
                  capabilities: [],
                  remaining: 9,
                },
              ],
              profiles: [
                {
                  identifier: "com.example.app.abc123",
                  expires: "2026-09-19T14:31:34Z",
                  uuid: "synthetic-uuid",
                },
              ],
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
          if (cmd === "installation_status")
            return (window as any).__jobResult ?? null;
          if (cmd === "prepare_install") {
            (window as any).__installPrepared = { path: args.path };
            return (window as any).__reviewResult;
          }
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
            if ((window as any).__withWatchApp)
              return {
                ...report,
                bundles: [
                  ...report.bundles,
                  {
                    ...report.bundles[0],
                    path: "Payload/Test.app/Watch/Watch.app",
                    kind: "Watch app",
                    name: "Synthetic Watch App",
                    identifier: "test.synthetic.watch",
                  },
                ],
              };
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
    page.getByRole("button", { name: "Re-sign IPA" }),
  ).toBeDisabled();
  await expect(page.getByLabel("Signing team", { exact: true })).toBeDisabled();
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
    page.getByRole("button", { name: "Re-sign IPA" }),
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
  await expect(page.getByLabel("Physical iPhone", { exact: true })).toHaveValue(
    "1",
  );
  await expect(page.getByText("Synthetic pairing verified.")).toBeVisible();
  await page.evaluate(() => {
    (window as any).__deviceResult = {
      devices: [],
      service_available: true,
      message: null,
    };
  });
  await page.getByRole("button", { name: "Refresh devices" }).click();
  await expect(page.getByLabel("Physical iPhone", { exact: true })).toHaveValue(
    "",
  );
  await expect(
    page.getByLabel("Physical iPhone", { exact: true }),
  ).toBeDisabled();
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
    page.getByRole("button", { name: "Re-sign IPA" }),
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
  await expect(page.getByLabel("Signing team", { exact: true })).toHaveValue(
    "",
  );
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM2");
  await page.getByRole("button", { name: "Change Signing team" }).click();
  await expect(page.getByLabel("Signing team", { exact: true })).toHaveValue(
    "TEAM2",
  );
  await expect(
    page.getByRole("button", { name: "Re-sign IPA" }),
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
  await expect(page.getByLabel("Signing team", { exact: true })).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Sign in to Apple" }),
  ).toBeDisabled();
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await expect(
    page.getByRole("button", { name: "Sign in to Apple" }),
  ).toBeEnabled();
});

test("provisioning replaces unverified findings with what the team established", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
  // Before provisioning, the panel can only say the identity is unknown.
  await expect(page.getByText("Signing identity required")).toBeVisible();
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM2");
  await page
    .getByLabel("I understand ten identifiers per seven days", { exact: false })
    .check();
  await page
    .getByRole("button", { name: "Prepare identifiers & profiles" })
    .click();

  // Afterwards it states what Apple decided, against the identifier that will install.
  await expect(
    page.getByText("Identifiers and profiles prepared"),
  ).toBeVisible();
  await expect(
    page.locator(".findings h4", { hasText: "Push notifications" }).first(),
  ).toBeVisible();
  await expect(
    page.getByText("Push notifications stop working in the re-signed app."),
  ).toBeVisible();
  await expect(
    page
      .locator(".findings code", { hasText: "com.example.app.abc123" })
      .first(),
  ).toBeVisible();
  // The contradicted finding is replaced outright, not left beside the answer.
  await expect(page.getByText("Signing identity required")).toHaveCount(0);
  // The seven-day expiry is a fact about the prepared build, not something left to configure.
  await expect(page.getByText("Every seven days")).toBeVisible();
  // The action bar shows the profile that will govern the build, not the company one.
  await expect(page.getByText("Prepared profile expiration")).toBeVisible();

  // Changing the IPA invalidates all of it rather than describing a build that is gone.
  await page.getByRole("button", { name: "Change", exact: true }).click();
  await expect(page.getByText("Identifiers and profiles prepared")).toHaveCount(
    0,
  );
});

test("the signed-in account and sign out live in the header", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await expect(page.getByText("Local workspace")).toBeVisible();
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  const header = page.locator("header");
  await expect(header.getByText("test@example.invalid")).toBeVisible();
  await expect(page.getByText("Local workspace")).toHaveCount(0);
  await header.getByRole("button", { name: "Sign out" }).click();
  // Signing out from the header returns the whole interface to the signed-out state.
  await expect(page.getByText("Local workspace")).toBeVisible();
  await expect(page.getByLabel("Apple account email")).toBeVisible();
});

test("the account panel keeps its content off the panel border", async ({
  page,
}) => {
  await nativeMock(page, "success");
  // Text flush against the border reads as a broken layout; every panel keeps the same inset.
  for (const [container, label] of [
    ["section.accounts", "Apple account"],
  ] as const) {
    const section = page.locator(container);
    const panel = await section.boundingBox();
    const heading = await section.getByText(label).first().boundingBox();
    expect(panel && heading && heading.x - panel.x).toBeGreaterThanOrEqual(16);
  }
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
  await expect(page.getByLabel("Physical iPhone", { exact: true })).toHaveValue(
    "1",
  );
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
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM2");
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
    name: "Get development certificate",
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

  // A refusal from the backend reaches the person intact: it names the cause and what to do.
  await page.evaluate(() => {
    (window as any).__certificateFailure =
      "Apple refused the request: this team already has 2 active development certificate(s), its maximum. Revoke one at developer.apple.com if it is no longer in use.";
  });
  // The step collapsed once it succeeded, so asking again means reopening it, as a person would.
  await page
    .getByRole("button", { name: "Change Development certificate" })
    .click();
  await certificate.click();
  await expect(
    page.getByText("already has 2 active development certificate", {
      exact: false,
    }),
  ).toBeVisible();
  await page.evaluate(() => {
    (window as any).__certificateFailure = undefined;
  });

  // Changing the team invalidates the result rather than carrying it across. The step collapsed
  // when it was satisfied, so reopening it is how a person gets back to the control.
  await page.getByRole("button", { name: "Change Signing team" }).click();
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM1");
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
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM1");
  await page.getByRole("button", { name: "Change Signing team" }).click();
  await page.getByRole("button", { name: "Refresh teams" }).click();
  await expect(page.getByLabel("Signing team", { exact: true })).toBeDisabled();
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
  // Local support now fails during sign-in itself, and the failure must be reported as such
  // without offering any remote alternative.
  await page.evaluate(() => {
    (window as any).__accountSignInResult = {
      stage: "failed",
      account: null,
      teams: [],
      selected_team: null,
      challenge: null,
      message:
        "Sign-in setup failed before account verification. Local macOS authentication support failed.",
    };
  });
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await expect(
    page.getByText("Local macOS authentication support failed.", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(
    page.getByText("ani.stikstore.app", { exact: false }),
  ).toHaveCount(0);
});

test("a Watch app must be decided before any identifier is registered", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__withWatchApp = true;
  });
  await nativeMock(page, "success");
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM2");
  await page
    .getByLabel("I understand ten identifiers per seven days", { exact: false })
    .check();

  // Acknowledged, but the Watch app is still undecided, so nothing may be registered.
  const prepare = page.getByRole("button", {
    name: "Prepare identifiers & profiles",
  });
  await expect(prepare).toBeDisabled();
  await expect(
    page.getByText("Choose what happens to the Watch app."),
  ).toBeVisible();

  await page.getByLabel("Watch app", { exact: true }).selectOption("remove");
  await expect(prepare).toBeEnabled();
  await prepare.click();
  // The choice reaches the backend, which decides what it means for the plan.
  await expect
    .poll(async () =>
      page.evaluate(() => (window as any).__provisioningRequested?.watch),
    )
    .toBe("remove");
});

test("signing produces a separate build and the installer moves to it", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
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
  await expect(page.getByLabel("Physical iPhone", { exact: true })).toHaveValue(
    "1",
  );
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM2");

  // Nothing may be signed before there is a certificate and Apple has returned profiles.
  const sign = page.getByRole("button", { name: "Re-sign IPA" });
  await expect(sign).toBeDisabled();
  await page
    .getByLabel("I understand this uses one of the team's", { exact: false })
    .check();
  await page
    .getByRole("button", { name: "Get development certificate" })
    .click();
  await expect(sign).toBeDisabled();
  await page
    .getByLabel("I understand ten identifiers per seven days", { exact: false })
    .check();
  await page
    .getByRole("button", { name: "Prepare identifiers & profiles" })
    .click();
  await expect(sign).toBeEnabled();
  await sign.click();

  // The result names the build that was produced, not the one that was chosen.
  await expect(page.getByText("Re-signed build expires")).toBeVisible();
  await expect(
    page.getByText("Re-signed 3 bundle(s), removed 1"),
  ).toBeVisible();
  await expect(page.getByText("Install the signed build")).toBeVisible();
  // The record of what happened is in the interface, not only in a terminal.
  await page.getByText("What re-signing did").click();
  await expect(
    page.getByText("signed main app as com.example.app.abc123"),
  ).toBeVisible();
  await expect
    .poll(async () =>
      page.evaluate(() => (window as any).__signRequested?.path),
    )
    .toBe("/synthetic/Test.ipa");

  // The installer reviews the signed file, leaving the original alone.
  await page.getByRole("button", { name: "Review installation" }).click();
  await expect
    .poll(async () =>
      page.evaluate(() => (window as any).__installPrepared?.path),
    )
    .toBe("/synthetic/signed/Test-TEAM2.ipa");

  // A free-team build will not launch until its certificate is trusted on the device, so the
  // one manual step left is stated where the install finishes.
  await page.evaluate(() => {
    (window as any).__reviewResult = {
      token: "synthetic-review",
      app_name: "Synthetic Test App",
      bundle_id: "com.example.app.abc123",
      version: "1.0",
      device_name: "Synthetic iPhone",
      size_bytes: 1048576,
      sha256: "synthetic-fingerprint",
      existing_app: null,
      blockers: [],
      notes: [],
    };
    (window as any).__jobResult = {
      id: "job",
      stage: "installed",
      message: "iOS reported installation complete.",
      transferred_bytes: 10,
      total_bytes: 10,
      device_percent: null,
      cleanup_pending: false,
    };
  });
  await page.getByRole("button", { name: "Review installation" }).click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install signed IPA" }).click();
  await expect(
    page.getByText("Trust the developer on the iPhone"),
  ).toBeVisible();
});

test("withdrawing a certificate is offered only when it is the only way forward", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.evaluate(() => {
    (window as any).__certificateFailure =
      "Apple refused the request: this team already holds one active development certificate, which is its maximum, and none of them certifies this Mac's signing key.";
  });
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM2");

  // Not offered until Apple has actually refused for that reason.
  await expect(
    page.getByRole("button", { name: "Withdraw the team's certificate" }),
  ).toHaveCount(0);
  await page
    .getByLabel("I understand this uses one of the team's", { exact: false })
    .check();
  await page
    .getByRole("button", { name: "Get development certificate" })
    .click();

  const withdraw = page.getByRole("button", {
    name: "Withdraw the team's certificate",
  });
  await expect(withdraw).toBeVisible();
  // Irreversible, so it waits for its own acknowledgement.
  await expect(withdraw).toBeDisabled();
  await page
    .getByLabel("I understand withdrawing this team's certificate", {
      exact: false,
    })
    .check();
  await expect(withdraw).toBeEnabled();
  await withdraw.click();
  await expect(
    page.getByText("certificate(s) were withdrawn", { exact: false }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as any).__withdrawRequested?.acknowledged,
    ),
  ).toBe(true);
});

test("a signed build says how to trust it before it will launch", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.evaluate(() => {
    (window as any).__jobResult = {
      id: "job",
      stage: "installed",
      message: "iOS reported installation complete.",
      transferred_bytes: 10,
      total_bytes: 10,
      device_percent: null,
      cleanup_pending: false,
    };
    (window as any).__reviewResult = {
      token: "synthetic-review",
      app_name: "Synthetic Test App",
      bundle_id: "test.synthetic",
      version: "1.0",
      device_name: "Synthetic iPhone",
      size_bytes: 1048576,
      sha256: "synthetic-fingerprint",
      existing_app: null,
      blockers: [],
      notes: [],
    };
  });
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
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
  await expect(page.getByLabel("Physical iPhone", { exact: true })).toHaveValue(
    "1",
  );

  // An unchanged company build is already trusted on a provisioned device: no instruction.
  await page.getByRole("button", { name: "Review installation" }).click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install unchanged IPA" }).click();
  await expect(
    page.getByText("iOS reported installation complete").first(),
  ).toBeVisible();
  await expect(page.getByText("Trust the developer on the iPhone")).toHaveCount(
    0,
  );
});

test("the device log is offered only for a signed build and keeps only its lines", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.getByRole("button", { name: /Drop your IPA/ }).click();
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
  await expect(page.getByLabel("Physical iPhone", { exact: true })).toHaveValue(
    "1",
  );

  // Nothing to diagnose before there is a build Orbiter signed.
  await expect(page.getByText("Device log")).toHaveCount(0);

  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();
  await page.getByLabel("Signing team", { exact: true }).selectOption("TEAM2");
  await page
    .getByLabel("I understand this uses one of the team's", { exact: false })
    .check();
  await page
    .getByRole("button", { name: "Get development certificate" })
    .click();
  await page
    .getByLabel("I understand ten identifiers per seven days", { exact: false })
    .check();
  await page
    .getByRole("button", { name: "Prepare identifiers & profiles" })
    .click();
  await page.getByRole("button", { name: "Re-sign IPA" }).click();
  await expect(page.getByText("Re-signed build expires")).toBeVisible();

  await page
    .getByRole("button", { name: "Capture while you reproduce it" })
    .click();
  await expect(
    page.getByText("social feed request refused", { exact: false }),
  ).toBeVisible();
  // The rest of the device's log is counted and discarded, not shown.
  await expect(
    page.getByText("812 about the rest of the device were discarded", {
      exact: false,
    }),
  ).toBeVisible();
  // The capture is scoped to the signed identifier, not to the device at large.
  expect(
    await page.evaluate(() => (window as any).__logRequested?.subjects),
  ).toContain("com.example.app.abc123");
});

test("help explains the losses without the panels having to", async ({
  page,
}) => {
  await nativeMock(page, "success");
  // Closed until asked for: it must not occupy the window by default.
  await expect(page.getByRole("dialog", { name: "Help" })).toHaveCount(0);
  await page.getByRole("button", { name: "Help" }).click();
  const help = page.getByRole("dialog", { name: "Help" });
  await expect(help).toBeVisible();
  // The four capability losses and the seven-day rule are stated in one place.
  await expect(help.getByText("Push notifications")).toBeVisible();
  await expect(help.getByText("The seven-day limit")).toBeVisible();
  await expect(
    help.getByText("Untrusted Developer", { exact: false }).first(),
  ).toBeVisible();
  // The work stays visible beside it rather than being covered.
  await expect(
    page.getByRole("button", { name: /Drop your IPA/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Close help" }).click();
  await expect(page.getByRole("dialog", { name: "Help" })).toHaveCount(0);
});

test("a finished step collapses to its result and can be reopened", async ({
  page,
}) => {
  await nativeMock(page, "success");
  await page.getByLabel("Apple account email").fill("test@example.invalid");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page
    .getByLabel("I agree to authenticate directly with Apple", { exact: false })
    .check();
  await page.getByRole("button", { name: "Sign in to Apple" }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify code" }).click();

  // Unfinished: the control is there and there is nothing to collapse.
  const select = page.getByLabel("Signing team", { exact: true });
  await expect(select).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Change Signing team" }),
  ).toHaveCount(0);

  await select.selectOption("TEAM2");
  // Finished: the control gives way to what it established.
  await expect(select).toHaveCount(0);
  await expect(page.locator(".stage-summary").first()).toContainText(
    "Free personal team",
  );

  // And it is never a dead end: the choice can be revisited.
  await page.getByRole("button", { name: "Change Signing team" }).click();
  await expect(select).toBeVisible();
  await expect(select).toHaveValue("TEAM2");
});
