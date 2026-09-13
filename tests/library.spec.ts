import { test, expect, type Page } from "@playwright/test";

async function mock(page: Page) {
  await page.addInitScript(() => {
    const w = window as any;
    w.isTauri = true;
    const fresh = {
      apps: [],
      artifacts: [],
      devices: [],
      attempts: [],
      storage_bytes: 0,
    };
    let data = JSON.parse(
      localStorage.getItem("test-library") ?? JSON.stringify(fresh),
    );
    const save = () =>
      localStorage.setItem("test-library", JSON.stringify(data));
    w.__library = data;
    w.__calls = [];
    w.__files = ["/test/One.ipa"];
    let current: any = null;
    let held: any = null;
    const callbacks = new Map<number, any>();
    let callbackId = 0;
    const report = {
      size_bytes: 1000,
      main_path: "Payload/Test.app",
      icon_data_url: null,
      bundles: [
        {
          path: "Payload/Test.app",
          kind: "Main app",
          name: "Library App",
          identifier: "test.library",
          version: "preview",
          build: "alpha",
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
      findings: [],
    };
    const listeners = new Map<number, { event: string; handler: number }>();
    let listenerId = 0;
    w.__hasDropListener = () =>
      [...listeners.values()].some(
        (listener) => listener.event === "tauri://drag-drop",
      );
    w.__drag = (type: string, paths: string[] = []) => {
      for (const [id, listener] of listeners)
        if (listener.event === `tauri://drag-${type}`)
          callbacks.get(listener.handler)?.({
            event: listener.event,
            id,
            payload: { paths, position: { x: 100, y: 100 } },
          });
    };
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    w.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main" },
      },
      transformCallback: (fn: any) => {
        callbacks.set(++callbackId, fn);
        return callbackId;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (cmd: string, args: any) => {
        w.__calls.push({ cmd, args });
        if (cmd === "plugin:event|listen") {
          listeners.set(++listenerId, args);
          return listenerId;
        }
        if (cmd === "plugin:event|unlisten") {
          listeners.delete(args.eventId);
          return;
        }
        if (cmd === "account_status")
          return {
            stage: "signed_out",
            account: null,
            teams: [],
            selected_team: null,
            challenge: null,
            message: "",
          };
        if (cmd === "renewal_status") return w.__legacyRenewal ?? null;
        if (cmd === "installation_status") return current;
        if (cmd === "discover_devices")
          return {
            devices: [
              {
                id: 1,
                name: "My iPhone",
                product_type: "iPhoneTest",
                ios_version: "18.0",
                connection: "USB",
                state: "paired",
                message: "Pairing verified.",
              },
            ],
            service_available: true,
            message: null,
          };
        if (cmd === "plugin:dialog|open") return w.__files;
        if (cmd === "library_list") {
          if (w.__corrupt)
            throw "Library storage is corrupt. Restore the manifest.";
          return JSON.parse(JSON.stringify(data));
        }
        if (cmd === "library_import") {
          if (args.path.includes("Bad"))
            throw "Invalid IPA. Choose a readable archive.";
          const previous = data.artifacts.find(
            (a: any) => a.testPath === args.path && !a.deleted,
          );
          if (previous)
            return {
              app_id: previous.app_id,
              artifact_id: previous.id,
              duplicate: true,
            };
          const id = `version-${data.artifacts.length + 1}`;
          if (!data.apps.length)
            data.apps.push({
              id: "app-1",
              identifier: "test.library",
              name: "Library App",
              icon_data_url: null,
              added_unix: 100,
            });
          data.artifacts.push({
            id,
            app_id: "app-1",
            source_id: null,
            sha256: id.repeat(8),
            name: "Library App",
            identifier: "test.library",
            version: "preview",
            build: "alpha",
            size_bytes: 1000,
            added_unix: data.artifacts.length + 100,
            expires: null,
            team_tag: null,
            watch: null,
            marker: null,
            deleted: false,
            testPath: args.path,
          });
          data.storage_bytes += 1000;
          save();
          return { app_id: "app-1", artifact_id: id, duplicate: false };
        }
        if (cmd === "library_open") {
          const artifact = data.artifacts.find(
            (a: any) => a.id === args.artifactId && !a.deleted,
          );
          if (!artifact) throw "Managed IPA is missing.";
          return {
            artifact,
            report: {
              ...report,
              icon_data_url: w.__workspaceIcon ?? report.icon_data_url,
            },
            path: `/managed/${artifact.id}.ipa`,
          };
        }
        if (cmd === "library_remove") {
          if (args.artifactId)
            data.artifacts.forEach((a: any) => {
              if (a.id === args.artifactId || a.source_id === args.artifactId)
                a.deleted = true;
            });
          else {
            data.apps = [];
            data.artifacts = [];
            data.attempts = [];
          }
          save();
          return;
        }
        if (cmd === "library_prepare_install") {
          held = data.artifacts.find((a: any) => a.id === args.artifactId);
          return {
            token: "review-1",
            app_name: held.name,
            bundle_id: held.identifier,
            version: held.version,
            device_name: "My iPhone",
            size_bytes: held.size_bytes,
            sha256: held.sha256,
            existing_app: null,
            blockers: [],
            notes: [],
          };
        }
        if (cmd === "execute_install") {
          current = {
            id: args.token,
            stage: "installing",
            message: "Installing reviewed build.",
            transferred_bytes: 1000,
            total_bytes: 1000,
            device_percent: null,
            cleanup_pending: false,
          };
          data.devices = [
            { id: "salted-device", name: "My iPhone", last_seen_unix: 200 },
          ];
          const attempt = {
            id: args.token,
            app_id: held.app_id,
            artifact_id: held.id,
            device_id: "salted-device",
            app_name: held.name,
            identifier: held.identifier,
            version: held.version,
            build: held.build,
            sha256: held.sha256,
            signed: !!held.source_id,
            expires: held.expires,
            started_unix: 200,
            finished_unix: null,
            stage: "installing",
            message: current.message,
          };
          data.attempts.push(attempt);
          save();
          if (w.__holdInstall)
            await new Promise((resolve) => {
              w.__finishInstall = resolve;
            });
          current = {
            ...current,
            stage: "installed",
            message: "iOS reported completion.",
          };
          Object.assign(attempt, {
            stage: "installed",
            message: current.message,
            finished_unix: 201,
          });
          save();
          return current;
        }
        if (cmd === "discard_install") return;
        if (cmd === "cancel_install") return false;
        throw `Unexpected IPC: ${cmd}`;
      },
    };
    w.__addSigned = () => {
      const source = data.artifacts[0];
      data.artifacts.push({
        ...source,
        id: "signed-1",
        source_id: source.id,
        identifier: "test.library.signed",
        sha256: "b".repeat(64),
        expires: "2099-01-01T00:00:00Z",
        team_tag: "team-tag",
        watch: "remove",
        marker: "test",
        added_unix: 300,
      });
      save();
      window.dispatchEvent(new Event("library-changed"));
    };
  });
  await page.goto("/");
}
async function imported(page: Page) {
  await page.getByRole("button", { name: "Import IPA", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Library App", exact: true }),
  ).toBeVisible();
}

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
    page.getByRole("heading", { name: "Signing & installation" }),
  ).toBeVisible();
  await page.getByLabel("Apple account email").fill("local@example.invalid");
  await page.getByRole("link", { name: "IPAs", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "IPAs", exact: true }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Resume Library App" }).click();
  await expect(page.getByLabel("Apple account email")).toHaveValue(
    "local@example.invalid",
  );
  await page.getByRole("link", { name: "Back to library" }).click();
  await page.getByRole("button", { name: "Import IPA", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Signing & installation" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Back to library" }).click();
  await expect(
    page.getByRole("list", { name: "Import results" }),
  ).toContainText("Already saved");
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toHaveCount(1);
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
    page.getByRole("heading", { name: "Install the signed build" }),
  ).toBeVisible();
  await expect(page.getByLabel("Apple account email")).not.toBeVisible();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  const install = page.getByRole("button", { name: "Install signed IPA" });
  await expect(install).toBeDisabled();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await install.click();
  await expect(
    page.getByText("iOS reported installation complete", { exact: true }),
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
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install signed IPA" }).click();
  await page.getByRole("link", { name: "Back to library" }).click();
  await expect(
    page.getByRole("button", { name: "Remove app and history" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("link", { name: "Open version", exact: true }),
  ).toHaveAttribute("aria-disabled", "true");
  await page.getByRole("link", { name: /Operation in progress/ }).click();
  await expect(
    page.getByRole("heading", { name: "Install the signed build" }),
  ).toBeVisible();
  await page.evaluate(() => (window as any).__finishInstall());
  await expect(
    page.getByText("iOS reported installation complete", { exact: true }),
  ).toBeVisible();
});

test("version deletion retains history; whole-app removal explicitly removes history", async ({
  page,
}) => {
  await mock(page);
  await imported(page);
  await page.getByRole("link", { name: "Open version", exact: true }).click();
  await page
    .getByRole("button", { name: "Review installation", exact: true })
    .click();
  await page
    .getByRole("checkbox", { name: /I authorize installation/ })
    .check();
  await page.getByRole("button", { name: "Install unchanged IPA" }).click();
  await expect(
    page.getByText("iOS reported installation complete", { exact: true }),
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
  await expect(
    page.getByRole("heading", { name: "Legacy renewal information" }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "This older record has no saved artifact or device association.",
      { exact: false },
    ),
  ).toBeVisible();
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
  await page.getByRole("link", { name: "IPAs", exact: true }).click();
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
    (window as any).__library.apps[0].icon_data_url =
      "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    (window as any).__workspaceIcon = (
      window as any
    ).__library.apps[0].icon_data_url;
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
    page.locator(".card.source").getByRole("img", { name: "Library App icon" }),
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
    page.getByRole("heading", { name: "Signing & installation" }),
  ).toBeVisible();
});
