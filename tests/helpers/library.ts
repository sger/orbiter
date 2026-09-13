import { expect, type Page } from "@playwright/test";

export async function mock(page: Page) {
  await page.addInitScript(() => {
    const w = window as any;
    w.isTauri = true;
    const fresh = {
      apps: [],
      artifacts: [],
      devices: [],
      attempts: [],
      expiries: [],
      storage_bytes: 0,
      unreferenced_bytes: 0,
    };
    let data = JSON.parse(
      localStorage.getItem("test-library") ?? JSON.stringify(fresh),
    );
    // A snapshot saved before this field existed still has to open, exactly as a manifest does.
    data.expiries ??= [];
    data.unreferenced_bytes ??= 0;
    const signedOut = {
      stage: "signed_out",
      account: null,
      teams: [],
      selected_team: null,
      challenge: null,
      message: "",
    };
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
        // Enough of the Apple account to reach a build that could actually be re-signed. The
        // account flow itself is covered in interface.spec.ts; here it exists so the expiry
        // banner's Re-sign can be driven the same way a person would.
        if (cmd === "library_preparation_status")
          return (
            w.__preparationStatus ?? {
              stage: "",
              message: "",
              completed: [],
              artifact_id: null,
            }
          );
        if (cmd === "library_discard_preparation") return;
        if (cmd === "account_sign_out") return (w.__account = signedOut);
        if (cmd === "account_refresh_teams") return w.__account;
        if (cmd === "account_answer")
          return (w.__account = {
            ...w.__account,
            stage: "signed_in",
            challenge: null,
          });
        if (cmd === "account_withdraw_certificates") return "Withdrawn";
        if (cmd === "library_review_preparation") {
          if (w.__planError) throw w.__planError;
          return (w.__plan = {
            token: "prepare-token",
            artifact_id: args.artifactId,
            device_name: "My iPhone",
            registration: !w.__reuse,
            certificate: !w.__reuse,
            provisioning: !w.__reuse,
            marker: args.marker,
            plan: {
              new_main_identifier: "test.library.signed",
              blockers: [],
              consequences: ["The signed app uses a separate identifier."],
              app_ids_required: 1,
              bundles: [],
            },
          });
        }
        if (cmd === "library_execute_preparation") {
          if (w.__prepareError) throw w.__prepareError;
          w.__signRequested = args;
          if (w.__holdSign)
            await new Promise((resolve) => {
              w.__finishSign = resolve;
            });
          w.__addSigned();
          return {
            signed: { path: "/managed/signed-1.ipa" },
            artifact: data.artifacts.find((a: any) => a.id === "signed-1"),
          };
        }
        if (cmd === "account_status") return w.__account ?? signedOut;
        if (cmd === "account_sign_in") {
          // A sign-in Apple refused in a way Orbiter could not classify. The command resolves
          // with a failed view rather than rejecting, which is what the real one does.
          if (w.__signInFailure) return (w.__account = w.__signInFailure);
          return (w.__account = {
            stage: w.__challenge ? "two_factor" : "signed_in",
            account: "test@example.invalid",
            selected_team: null,
            challenge: w.__challenge ?? null,
            message: "Signed in. Select a team.",
            teams: w.__teams ?? [
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
          w.__account.selected_team = args.id;
          return w.__account;
        }
        if (cmd === "account_register_device")
          return {
            registration: "registered",
            team_devices: 1,
            message: "This iPhone is now registered on the selected team.",
          };
        if (cmd === "account_request_certificate")
          return {
            reused: false,
            expires: "2099-01-01T00:00:00Z",
            active: 1,
            message: "A development certificate was issued.",
          };
        if (cmd === "library_prepare_provisioning")
          return {
            plan: {
              new_main_identifier: "test.library.signed",
              blockers: [],
              consequences: [],
              app_ids_required: 1,
              bundles: [],
            },
            app_ids: [
              {
                identifier: "test.library.signed",
                created: true,
                capabilities: [],
                remaining: 9,
              },
            ],
            profiles: [
              {
                identifier: "test.library.signed",
                expires: "2099-01-01T00:00:00Z",
                uuid: "synthetic-uuid",
              },
            ],
          };
        if (cmd === "library_sign") {
          w.__signRequested = {
            artifactId: args.artifactId,
            watch: args.watch,
            marker: args.marker,
          };
          w.__addSigned();
          return {
            signed: {
              team_tag: "team-tag",
              path: "/managed/signed-1.ipa",
              identifier: "test.library.signed",
              expires: "2099-01-01T00:00:00Z",
              expires_unix: 4070908800,
              bundles_signed: 1,
              removed: [],
              message: "A signed IPA was produced.",
              log: ["Extracting the archive"],
            },
            artifact: data.artifacts.find((a: any) => a.id === "signed-1"),
          };
        }
        if (cmd === "renewal_status") return w.__legacyRenewal ?? null;
        if (cmd === "installation_status") return w.__resume ?? current;
        if (cmd === "discover_devices")
          return {
            devices: w.__devices ?? [
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
        // Where the seven days stand. Rust decides every word of it, so a test sets the whole
        // record and asserts the page renders it rather than computing anything itself.
        if (cmd === "library_expiry") {
          return (
            data.expiries.find(
              (e: any) =>
                e.artifact_id === args.artifactId ||
                data.artifacts.some(
                  (a: any) =>
                    a.id === e.artifact_id && a.source_id === args.artifactId,
                ),
            ) ?? null
          );
        }
        if (cmd === "library_reclaim") {
          const freed = data.unreferenced_bytes;
          data.unreferenced_bytes = 0;
          save();
          return freed;
        }
        if (cmd === "library_icon") return (w.__icons ?? {})[args.sha] ?? null;
        if (cmd === "library_list") {
          // Either shape: a bare string from a command this refactor has not reached, or the
          // structured failure the refactored ones reject with.
          if (w.__corrupt)
            throw w.__corrupt === true
              ? "Library storage is corrupt. Restore the manifest."
              : w.__corrupt;
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
              bundles: w.__watch
                ? [
                    ...report.bundles,
                    {
                      ...report.bundles[0],
                      kind: "Watch app",
                      path: "Payload/Test.app/Watch/Test.app",
                    },
                  ]
                : report.bundles,
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
          if (w.__checkError) throw w.__checkError;
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
            readiness: held.source_id ? "direct" : (w.__readiness ?? "direct"),
            issues: [],
            blockers:
              !held.source_id && w.__readiness && w.__readiness !== "direct"
                ? ["Profile or device check needs attention"]
                : [],
            notes: [],
          };
        }
        if (cmd === "execute_install") {
          if (w.__installError) throw w.__installError;
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
            stage: w.__outcome ?? "installed",
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
export async function imported(page: Page) {
  await page.getByRole("button", { name: "Import IPA", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Library App", exact: true }),
  ).toBeVisible();
}
