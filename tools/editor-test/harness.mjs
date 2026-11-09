// The draft editor's page in WebKit, without the app: what the app would do is recorded (`sent`) or done by
// the tests (`JM.…`). See run.sh.
import { execFileSync } from "node:child_process";
import { webkit } from "playwright-core";

const APP = process.env.JUST_MAIL_APP || new URL("../../target/debug/just-mail", import.meta.url).pathname;

/// The page as the app makes it (`JUST_MAIL_EDITOR_PAGE`), with `html` as the draft's text.
export function pageHtml(theme, html = "") {
  const page = execFileSync(APP, { env: { ...process.env, JUST_MAIL_EDITOR_PAGE: theme, JUST_MAIL_LANG: "en" }, encoding: "utf8", maxBuffer: 1 << 26 });
  if (!page.includes('"html":""')) throw new Error("the app's page has no empty text to fill");
  // as the app embeds it: no `<` that could end the script
  return page.replace('"html":""', () => `"html":${JSON.stringify(html).replace(/</g, "\\u003c")}`);
}

export async function launch() {
  return webkit.launch();
}

/// An editor showing `html`. `sent` collects what the page reports to the app.
export async function editor(browser, html = "", { theme = "light", width = 720, height = 520 } = {}) {
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 2 });
  const page = await context.newPage();
  const sent = [];
  await page.exposeFunction("__sent", (message) => sent.push(JSON.parse(message)));
  await page.addInitScript(() => {
    window.ipc = { postMessage: (message) => window.__sent(message) };
  });
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  // loaded from an address, so the script above runs before the page's own
  await page.route("http://editor.test/", (route) => route.fulfill({ contentType: "text/html; charset=utf-8", body: pageHtml(theme, html) }));
  await page.goto("http://editor.test/");
  await page.waitForFunction(() => window.JM);
  return {
    page,
    sent,
    errors,
    /// The text as the mail would carry it: the last change reported.
    async html() {
      await page.evaluate(() => new Promise(requestAnimationFrame));
      const edits = sent.filter((m) => m.editor && m.editor.type === "edited");
      return edits.length ? edits.at(-1).editor.html : html;
    },
    /// The text as the page holds it.
    dom: () => page.evaluate(() => document.getElementById("text").innerHTML),
    /// Put the cursor at the end of the element matching `selector` in the text, or select all of it.
    async caret(selector, all = false) {
      await page.evaluate(([selector, all]) => {
        const node = document.querySelector(`#text ${selector}`), s = getSelection();
        document.getElementById("text").focus();
        s.selectAllChildren(node);
        if (!all) s.collapseToEnd();
      }, [selector, all]);
    },
    select: (selector) => page.evaluate((selector) => {
      const node = document.querySelector(`#text ${selector}`);
      document.getElementById("text").focus();
      getSelection().selectAllChildren(node);
    }, selector),
    button: (name) => page.click(`#bar [data-cmd="${name}"], #bar [data-menu="${name}"]`),
    /// Whether the bar's button is lit (the bar follows the cursor with the next frame).
    isOn: (name) => page.evaluate(async (name) => {
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return document.querySelector(`#bar [data-cmd="${name}"], #bar [data-menu="${name}"]`).classList.contains("on");
    }, name),
    type: (text) => page.keyboard.type(text),
    press: (key) => page.keyboard.press(key),
    close: () => context.close(),
  };
}
