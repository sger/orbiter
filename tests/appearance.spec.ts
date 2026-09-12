import { test, expect } from "@playwright/test";

test("appearance defaults to system, reacts to OS changes, and persists an explicit choice", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/#/settings");
  const root = page.locator("html");
  await expect(root).toHaveAttribute("data-theme", "light");
  await expect(page.getByRole("radio", { name: /^System/ })).toBeChecked();
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(root).toHaveAttribute("data-theme", "dark");
  await page.getByRole("radio", { name: /^Light/ }).check();
  await expect(root).toHaveAttribute("data-theme", "light");
  await page.reload();
  await expect(root).toHaveAttribute("data-theme", "light");
  await expect(page.getByRole("radio", { name: /^Light/ })).toBeChecked();
  await page.getByRole("radio", { name: /^Dark/ }).check();
  await page.emulateMedia({ colorScheme: "light" });
  await expect(root).toHaveAttribute("data-theme", "dark");
  await page.getByRole("link", { name: "IPAs", exact: true }).click();
  await expect(root).toHaveAttribute("data-theme", "dark");
  await expect(page.locator(".card.source")).toHaveCSS(
    "background-color",
    "rgb(34, 34, 34)",
  );
  await page.getByRole("button", { name: "What is stored" }).click();
  await expect(page.getByRole("dialog", { name: "Help" })).toHaveCSS(
    "background-color",
    "rgb(34, 34, 34)",
  );
  await page.getByRole("button", { name: "Close help" }).click();
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: /^System/ }).check();
  await expect(root).toHaveAttribute("data-theme", "light");
});

for (const mode of ["light", "dark"] as const) {
  test(`${mode} gray surfaces cover the workspace and responsive Settings`, async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: mode });
    await page.goto("/");
    const colors = await page
      .locator(".sidebar, .card.source, .text-field input, .select-control")
      .evaluateAll((elements) =>
        elements.map((element) => {
          const color = getComputedStyle(element).backgroundColor;
          return color.match(/\d+/g)!.slice(0, 3).map(Number);
        }),
      );
    for (const [r, g, b] of colors) {
      expect(r).toBe(g);
      expect(g).toBe(b);
    }
    await page.screenshot({
      path: `test-results/workspace-${mode}.png`,
      fullPage: true,
    });
    await page.getByRole("link", { name: "Settings", exact: true }).click();
    await expect(
      page.getByRole("link", { name: "Settings", exact: true }),
    ).toHaveAttribute("aria-current", "page");
    await page.getByRole("radio", { name: /^System/ }).focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByRole("radio", { name: /^Light/ })).toBeChecked();
    await page
      .getByRole("radio", { name: mode === "dark" ? /^Dark/ : /^Light/ })
      .check();
    await page.setViewportSize({ width: 780, height: 640 });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    await page.screenshot({
      path: `test-results/settings-${mode}.png`,
      fullPage: true,
    });
  });
}

test("invalid or unavailable preference storage leaves appearance usable", async ({
  page,
}) => {
  await page.addInitScript(() =>
    localStorage.setItem("orbiter.appearance", "invalid"),
  );
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/#/settings");
  await expect(page.getByRole("radio", { name: /^System/ })).toBeChecked();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.addInitScript(() => {
    Storage.prototype.getItem = () => {
      throw new Error("unavailable");
    };
    Storage.prototype.setItem = () => {
      throw new Error("unavailable");
    };
  });
  await page.reload();
  await page.getByRole("radio", { name: /^Light/ }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
});

test("subtle motion respects the system reduced-motion preference", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.goto("/#/settings");
  const option = page.locator(".appearance-option").first();
  await option.hover();
  await expect(option).toHaveCSS("transform", "matrix(1, 0, 0, 1, 0, -2)");
  await expect(page.locator('[data-page-active="true"]')).toHaveCSS(
    "animation-name",
    "content-reveal",
  );
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(option).toHaveCSS("transform", "none");
  await expect(option).toHaveCSS("transition-duration", "0s");
  await expect(page.locator('[data-page-active="true"]')).toHaveCSS(
    "animation-name",
    "none",
  );
  await page.getByRole("link", { name: "IPAs", exact: true }).click();
  await page.getByRole("button", { name: "What is stored" }).click();
  await expect(page.getByRole("dialog", { name: "Help" })).toHaveCSS(
    "animation-name",
    "none",
  );
});
