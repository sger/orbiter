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
