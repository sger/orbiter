import { test, expect } from "@playwright/test";

test("installation prototype presents direct, personal, paid, and unverified-team paths", async ({
  page,
}) => {
  await page.goto("/design/install-flow.html");
  await expect(
    page.getByRole("button", { name: "Check app & iPhone" }),
  ).toBeDisabled();
  await page.locator('[data-pick="app"]').first().click();
  await page.locator('[data-pick="device"]').first().click();
  await page.getByRole("button", { name: "Check app & iPhone" }).click();
  await expect(
    page.getByRole("heading", { name: "Ready for installation review" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Review installation", exact: false })
    .click();
  await expect(
    page.getByRole("button", { name: "Install on iPhone" }),
  ).toBeDisabled();
  await page.locator("#agree").check();
  await page.getByRole("button", { name: "Install on iPhone" }).click();
  await expect(
    page.getByRole("heading", { name: "Installing on Alex’s iPhone" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Preview completion" }).click();
  await expect(
    page.getByRole("heading", { name: "Installation complete" }),
  ).toBeVisible();
  for (const scenario of ["new", "paid"]) {
    await page.locator("#scenario").selectOption(scenario);
    await page.locator('#index [data-go="prepare"]').click();
    await expect(
      page.getByRole("heading", {
        name:
          scenario === "paid"
            ? "Orbit Studio · Developer Program"
            : "Alex · Personal Team",
      }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Prepare & sign" }),
    ).toBeDisabled();
    await page.locator("#agree").check();
    await expect(
      page.getByRole("button", { name: "Prepare & sign" }),
    ).toBeEnabled();
  }
  await page.locator("#scenario").selectOption("teams");
  await page.locator('#index [data-go="team"]').click();
  await page.locator('[data-team="paid"]').click();
  await expect(
    page.getByRole("heading", { name: "Orbit Studio · Developer Program" }),
  ).toBeVisible();
  await page.locator("#scenario").selectOption("unknown");
  await page.locator('#index [data-go="team"]').click();
  await expect(
    page.getByRole("heading", { name: "Confirm your team membership" }),
  ).toBeVisible();
  await expect(page.locator("#screen .actions .primary")).toBeDisabled();
});

test("capture installation screens for flow review", async ({ page }) => {
  await page.setViewportSize({ width: 1360, height: 1060 });
  await page.goto("/design/install-flow.html");
  for (const [scenario, screen, name] of [
    ["ready", "choose", "01-choose"],
    ["ready", "check", "02-ready"],
    ["new", "account", "03-account"],
    ["teams", "team", "04-teams"],
    ["new", "prepare", "05-personal"],
    ["paid", "prepare", "06-developer"],
    ["paid", "review", "07-review"],
    ["ready", "installing", "08-progress"],
    ["new", "done", "09-complete"],
    ["failure", "failed", "10-recovery"],
    ["unknown", "team", "11-unverified"],
    ["locked", "device", "12-device-help"],
  ]) {
    await page.locator("#scenario").selectOption(scenario);
    await page.locator(`#index [data-go="${screen}"]`).click();
    await page.screenshot({
      path: `design/screens/${name}.png`,
      fullPage: true,
      animations: "disabled",
    });
  }
  await page.getByRole("button", { name: "Light appearance" }).click();
  await page.locator("#scenario").selectOption("ready");
  await page.setViewportSize({ width: 780, height: 840 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "design/screens/13-light-compact.png",
    fullPage: true,
    animations: "disabled",
  });
});
