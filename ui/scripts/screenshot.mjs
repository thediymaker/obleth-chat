// Screenshots of the running interface, for checking a page by eye. Run inside the
// Playwright image (see README.md): signs in as admin, then shoots each path.
//   BASE=http://127.0.0.1:3101 PW=... node screenshot.mjs /projects /settings
import { chromium } from "playwright";
const base = process.env.BASE;
const paths = process.argv.slice(2);
const browser = await chromium.launch();
for (const theme of (process.env.THEMES || "dark,light").split(",")) {
  const page = await browser.newPage({ viewport: { width: Number(process.env.WIDTH || 1360), height: 900 }, colorScheme: theme });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.goto(base + "/", { waitUntil: "networkidle" });
  if (await page.locator("#username").count()) {
    await page.fill("#username", "admin");
    await page.fill("#password", process.env.PW || "local-test-password-1");
    await page.click("button[type=submit]");
    await page.waitForTimeout(1500);
  }
  for (const path of paths) {
    await page.goto(base + path, { waitUntil: "networkidle" });
    await page.waitForTimeout(800);
    const file = `${process.env.OUT || "."}/${theme}${path.replace(/[^a-z0-9]+/gi, "-") || "-home"}.png`;
    await page.screenshot({ path: file, fullPage: true });
    console.log("saved", file);
  }
  if (errors.length) console.log(theme, "errors:", JSON.stringify(errors));
  await page.close();
}
await browser.close();
