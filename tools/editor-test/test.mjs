// What the draft editor's page does, with real keys and clicks in WebKit. Run through run.sh.
import assert from "node:assert/strict";
import { editor, launch } from "./harness.mjs";

const only = process.argv[2];
const tests = [];
const test = (name, html, run, options) => tests.push({ name, html, run, options });
const has = (text, part) => assert.ok(text.includes(part), `expected\n  ${part}\nin\n  ${text}`);
const hasNot = (text, part) => assert.ok(!text.includes(part), `expected no\n  ${part}\nin\n  ${text}`);
// a 2×1 GIF
const GIF = "data:image/gif;base64,R0lGODlhAgABAIAAAAAAAP///yH5BAEAAAAALAAAAAACAAEAAAICBAoAOw==";
const CELL = "border:1px solid #d0d0d0;padding:4px 10px;vertical-align:top";
const TABLE = `<div>vor</div><table cellpadding="0" cellspacing="0" style="border-collapse:collapse"><tbody><tr><th style="${CELL};font-weight:bold">A</th><th style="${CELL};font-weight:bold">B</th></tr><tr><td style="${CELL}">1</td><td id="last" style="${CELL};text-align:right">2</td></tr></tbody></table><div>nach</div>`;

test("typing reports the text, ⌘B makes bold and the button shows it", "", async (e) => {
  await e.type("Hallo ");
  await e.press("Meta+b");
  assert.equal(await e.isOn("bold"), true);
  await e.type("Welt");
  has(await e.html(), "Hallo <b>Welt</b>");
  await e.press("Meta+b");
  await e.type("!");
  has(await e.html(), "<b>Welt</b>!");
  assert.equal(await e.isOn("bold"), false);
});

test("the bar's buttons work on the selection and keep it", "<div>eins <i id=w>zwei</i> drei</div>", async (e) => {
  await e.select("#w");
  await e.button("bold");
  await e.button("strike");
  await e.button("underline");
  const html = await e.html();
  for (const tag of ["<b>", "<strike>", "<u>"]) has(html, tag);
  assert.equal(await e.page.evaluate(() => getSelection().toString()), "zwei");
  assert.deepEqual([await e.isOn("bold"), await e.isOn("italic"), await e.isOn("strike")], [true, true, true]);
});

test("nothing is reported before something changes, and undo has nothing to undo", "<div>alt</div>", async (e) => {
  await e.caret("div");
  await e.press("Meta+z");
  assert.equal(e.sent.filter((m) => m.editor.type === "edited").length, 0);
  assert.deepEqual(e.sent.map((m) => m.editor.type).filter((t) => t !== "focused"), ["ready"]);
});

test("⌘Z takes back what was typed in one go, ⇧⌘Z does it again", "<div id=a>alt</div>", async (e) => {
  await e.caret("#a");
  await e.type(" neu");
  await e.press("Meta+b");
  await e.type("fett");
  has(await e.html(), "alt neu<b>fett</b>");
  await e.press("Meta+z");
  assert.equal(await e.html(), '<div id="a">alt neu</div>');
  await e.press("Meta+z");
  assert.equal(await e.html(), '<div id="a">alt</div>');
  await e.press("Meta+z");
  assert.equal(await e.html(), '<div id="a">alt</div>');
  await e.press("Meta+Shift+z");
  await e.press("Meta+Shift+z");
  has(await e.html(), "alt neu<b>fett</b>");
  // the cursor is back where the text ended
  await e.type("!");
  has(await e.html(), "<b>fett!</b>");
});

test("undo puts the cursor back where the change was, also for what the app inserted", "<div id=a>eins zwei</div>", async (e) => {
  await e.page.evaluate(() => {
    document.getElementById("text").focus();
    getSelection().collapse(document.getElementById("a").firstChild, 4);
  });
  await e.type("X");
  await e.press("ArrowRight");
  await e.type("Y");
  assert.equal(await e.html(), '<div id="a">einsX Yzwei</div>');
  await e.press("Meta+z");
  await e.press("Meta+z");
  await e.type("!");
  assert.equal(await e.html(), '<div id="a">eins! zwei</div>');
  await e.page.evaluate(() => JM.insert("<b>neu</b>"));
  has(await e.html(), "eins!<b>neu</b>");
  await e.press("Meta+z");
  assert.equal(await e.html(), '<div id="a">eins! zwei</div>');
});

test('"- " and "1. " at the start of a line begin lists, ↩ on an empty item ends them', "", async (e) => {
  await e.type("- eins");
  await e.press("Enter");
  await e.type("zwei");
  let html = await e.html();
  has(html, '<ul style="margin-top: 0px; margin-bottom: 0px;"><li>eins</li><li>zwei</li></ul>');
  assert.equal(await e.isOn("bullets"), true);
  await e.press("Enter");
  await e.press("Enter");
  await e.type("1. erstens");
  html = await e.html();
  has(html, "<li>erstens</li></ol>");
  hasNot(html, "1.");
  // taken back, the dash is a dash again
  await e.press("Meta+z");
  await e.press("Meta+z");
  has(await e.html(), "1.");
});

test("a dash in the middle of a line stays a dash", "<div id=a>Preis</div>", async (e) => {
  await e.caret("#a");
  await e.type(" - 20 %");
  assert.equal(await e.html(), '<div id="a">Preis - 20 %</div>');
});

test("⇥ and ⇧⇥ move a list item in and out", "<ul><li>eins</li><li id=b>zwei</li></ul>", async (e) => {
  await e.caret("#b");
  await e.press("Tab");
  has(await e.html(), '<li>eins</li><ul style="margin-top: 0px; margin-bottom: 0px;"><li id="b">zwei</li></ul>');
  await e.press("Shift+Tab");
  const html = await e.html();
  has(html, "<li>eins</li><li>zwei");
  assert.equal(html.match(/<ul/g).length, 1);
});

test("⇥ in plain text stays in the text, ⇧⇥ goes back to the field above", "<div id=a>Text</div>", async (e) => {
  await e.caret("#a");
  await e.press("Tab");
  await e.type("x");
  assert.equal(await e.html(), '<div id="a">Textx</div>');
  await e.press("Shift+Tab");
  assert.equal(e.sent.at(-1).editor.type, "leave");
});

test("a typed web address becomes a link, the full stop behind it does not", "", async (e) => {
  await e.type("siehe https://example.com/a?b=1. Und www.example.org");
  await e.press("Enter");
  await e.type("weiter");
  const html = await e.html();
  has(html, 'siehe <a href="https://example.com/a?b=1">https://example.com/a?b=1</a>. Und ');
  has(html, '<a href="https://www.example.org">www.example.org</a>');
  has(html, "<div>weiter</div>");
});

test("⌘K links the selection, shows the address again, and takes the link away", "<div>zum <i id=w>Shop</i> hier</div>", async (e) => {
  await e.select("#w");
  await e.press("Meta+k");
  await e.type("example.com/shop");
  await e.press("Enter");
  has(await e.html(), '<i id="w"><a href="https://example.com/shop">Shop</a></i>');
  await e.caret("a");
  assert.equal(await e.isOn("link"), true);
  await e.press("Meta+k");
  assert.equal(await e.page.inputValue("#pop input"), "https://example.com/shop");
  await e.type("mailto:a@example.com");
  await e.press("Enter");
  has(await e.html(), '<a href="mailto:a@example.com">Shop</a>');
  await e.caret("a");
  await e.press("Meta+k");
  await e.page.click('#pop button[title^="Remove"]');
  assert.equal(await e.html(), '<div>zum <i id="w">Shop</i> hier</div>');
});

test("⌘ and a click open a link in the browser", '<div>zum <a href="https://example.com/shop">Shop</a></div>', async (e) => {
  await e.page.click("#text a", { modifiers: ["Meta"] });
  assert.deepEqual(e.sent.at(-1), { open: "https://example.com/shop" });
});

test("an address that could run something is no link", "<div id=a>Text</div>", async (e) => {
  await e.select("#a");
  await e.press("Meta+k");
  await e.type("javascript:alert(1)");
  await e.press("Enter");
  assert.equal(await e.dom(), '<div id="a">Text</div>');
});

test("⌘K without a selection puts the address there as a link; a mail address becomes mailto:", "<div id=a>an </div>", async (e) => {
  await e.caret("#a");
  await e.press("Meta+k");
  await e.type("sam@carter.example");
  await e.press("Enter");
  has(await e.html(), '<a href="mailto:sam@carter.example">sam@carter.example</a>');
});

test("a pasted address links what is selected", "<div>zum <i id=w>Shop</i> hier</div>", async (e) => {
  await e.select("#w");
  await e.page.evaluate(() => JM.link("https://example.com/"));
  has(await e.html(), '<a href="https://example.com/">Shop</a>');
});

test("sizes: of the selection, of the line without one, and normal again", "<div id=a>Titel <b id=w>fett</b> Ende</div><div>zwei</div>", async (e) => {
  await e.select("#w");
  await e.button("size");
  await e.page.click("#pop .item >> text=Very large");
  has(await e.html(), '<b id="w"><span style="font-size: 15pt;">fett</span></b>');
  assert.equal(await e.page.evaluate(() => getSelection().toString()), "fett");
  // the whole line, the cursor stays
  await e.caret("#w span");
  await e.button("size");
  await e.page.click("#pop .item >> text=Large");
  let html = await e.html();
  has(html, '<div id="a"><span style="font-size: 13pt;">Titel <b id="w">fett</b> Ende</span></div><div>zwei</div>');
  await e.type("!");
  has(await e.html(), "fett!</b>");
  await e.button("size");
  assert.equal((await e.page.textContent("#pop .item:has(svg)")).trim(), "Large");
  await e.page.click("#pop .item >> text=Normal");
  html = await e.html();
  hasNot(html, "font-size");
  hasNot(html, "<span");
});

test("text color and marker go on and off", "<div>ein <i id=w>Wort</i> hier</div>", async (e) => {
  await e.select("#w");
  await e.button("color");
  await e.page.click("#pop .swatches:nth-of-type(2) .swatch:nth-child(2)");
  has(await e.html(), '<font color="#d93025">Wort</font>');
  await e.select("#w");
  await e.button("color");
  await e.page.click("#pop .swatches:nth-of-type(4) .swatch:nth-child(2)");
  has(await e.html(), "background-color: rgb(255, 242, 122)");
  await e.select("#w");
  await e.button("color");
  await e.page.click("#pop .swatches:nth-of-type(2) .swatch:nth-child(1)");
  hasNot(await e.html(), "<font");
  await e.select("#w");
  await e.button("color");
  await e.page.click("#pop .swatches:nth-of-type(4) .swatch:nth-child(1)");
  assert.equal(await e.html(), '<div>ein <i id="w">Wort</i> hier</div>');
});

test("a quote gets the style a mail needs, ↩ on an empty line ends it", "<div>eins</div><div id=b>zwei</div>", async (e) => {
  await e.caret("#b");
  await e.button("quote");
  assert.equal(await e.isOn("quote"), true);
  has(await e.html(), '<blockquote style="margin:0 0 0 4px;padding-left:10px;border-left:3px solid #d0d0d0">');
  await e.press("Enter");
  await e.type("drei");
  await e.press("Enter");
  await e.press("Enter");
  await e.type("danach");
  const html = await e.html();
  has(html, "drei</blockquote><div>danach</div>");
  assert.equal(await e.isOn("quote"), false);
  await e.caret("blockquote");
  await e.button("quote");
  has(await e.html(), "<div>eins</div><div>zwei</div><blockquote");
  // all of it selected: no quote is left
  await e.page.evaluate(() => getSelection().selectAllChildren(document.getElementById("text")));
  await e.button("quote");
  assert.equal(await e.html(), "<div>eins</div><div>zwei</div><div>drei</div><div>danach</div>");
});

test("a new table has a header row and the cursor in it; ⇥ walks the cells and adds a row", "<div id=a>Text</div>", async (e) => {
  await e.caret("#a");
  await e.button("table");
  await e.page.click("#pop .grid i:nth-child(8)"); // 2 columns, 2 rows
  await e.type("A");
  await e.press("Tab");
  await e.type("B");
  await e.press("Tab");
  await e.type("1");
  await e.press("Tab");
  await e.type("2");
  await e.press("Tab");
  await e.type("3");
  await e.press("Shift+Tab");
  await e.type("!");
  const html = await e.html();
  has(html, '<table cellpadding="0" cellspacing="0" style="border-collapse: collapse;"><tbody><tr><th style="border:1px solid #d0d0d0;padding:4px 10px;vertical-align:top;font-weight:bold;background-color:#f3f3f3;text-align:left">A</th>');
  has(html, '<td style="border:1px solid #d0d0d0;padding:4px 10px;vertical-align:top;text-align:left">1</td>');
  assert.equal(html.replace(/<(t[hd]|table)[^>]*>/g, "<$1>").match(/<table>.*<\/table>/)[0],
    "<table><tbody><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2!</td></tr><tr><td>3</td><td><br></td></tr></tbody></table>");
  hasNot(html, "data-new");
  assert.equal(await e.isOn("table"), true);
});

test("rows and columns come and go, undo brings them back", TABLE, async (e) => {
  const cells = async () => (await e.html()).replace(/<(t[hd])[^>]*>/g, "<$1>").match(/<tbody>(.*)<\/tbody>/)?.[1];
  const menu = async (label) => {
    await e.button("table");
    await e.page.click(`#pop .item >> text=${label}`);
  };
  await e.caret("#last");
  await menu("Row below");
  await e.type("x");
  assert.equal(await cells(), "<tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr><tr><td><br></td><td>x</td></tr>");
  // the new cell keeps its column's alignment
  has(await e.html(), 'vertical-align:top;text-align:right">x</td>');
  await e.caret("#last");
  await menu("Column to the left");
  assert.equal(await cells(), "<tr><th>A</th><th><br></th><th>B</th></tr><tr><td>1</td><td><br></td><td>2</td></tr><tr><td><br></td><td><br></td><td>x</td></tr>");
  await e.press("Meta+z");
  assert.equal(await cells(), "<tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr><tr><td><br></td><td>x</td></tr>");
  await e.caret("#last");
  await menu("Delete row");
  await menu("Delete column");
  assert.equal(await cells(), "<tr><th>A</th></tr><tr><td><br></td></tr>");
  await menu("Delete table");
  assert.equal(await e.html(), "<div>vor</div><div>nach</div>");
  await e.type("!");
  assert.equal(await e.html(), "<div>vor</div><div>!nach</div>");
});

test("pictures show from their bytes and travel as cid:, a click frames them, the corner sizes them", '<div id=a>Bild: </div><img src="cid:Later@jm-1" alt="b" width="40" height="20">', async (e) => {
  assert.equal(await e.page.getAttribute("#text img", "class"), "loading");
  await e.page.evaluate((url) => JM.pictures({ "later@jm-1": url, "new@jm-1": url }), GIF);
  assert.equal(await e.page.getAttribute("#text img", "class"), "");
  await e.caret("#a");
  await e.page.evaluate(() => JM.insert('<img src="cid:new@jm-1" alt="Bild 1.png" width="200" height="100" style="max-width:100%;height:auto">'));
  let html = await e.html();
  has(html, '<img src="cid:new@jm-1" alt="Bild 1.png" width="200" height="100" style="max-width: 100%; height: auto;">');
  hasNot(html, "data:");
  hasNot(html, "data-cid");
  await e.page.click('#text img[alt="Bild 1.png"]');
  assert.equal(await e.page.isVisible("#frame"), true);
  const handle = await e.page.locator("#frame i").boundingBox();
  await e.page.mouse.move(handle.x + 5, handle.y + 5);
  await e.page.mouse.down();
  await e.page.mouse.move(handle.x - 95, handle.y + 5, { steps: 4 });
  await e.page.mouse.up();
  has(await e.html(), 'alt="Bild 1.png" width="100" height="50"');
  // ⌫ takes the framed picture away, the frame with it
  await e.press("Backspace");
  hasNot(await e.html(), "new@jm-1");
  assert.equal(await e.page.isVisible("#frame"), false);
  await e.press("Meta+z");
  has(await e.html(), 'src="cid:new@jm-1"');
  assert.match(await e.page.getAttribute('#text img[alt="Bild 1.png"]', "src"), /^data:image\/gif/);
  await e.page.evaluate(() => JM.drop("new@jm-1"));
  hasNot(await e.html(), "new@jm-1");
});

test("⌘V asks the app, ⇧⌘V for text only; a file dragged over the text shows", "<div id=a>Text</div>", async (e) => {
  await e.caret("#a");
  await e.press("Meta+v");
  assert.deepEqual(e.sent.at(-1).editor, { page: 1, type: "paste", plain: false });
  await e.press("Meta+Shift+v");
  assert.deepEqual(e.sent.at(-1).editor, { page: 1, type: "paste", plain: true });
  await e.button("picture");
  assert.equal(e.sent.at(-1).editor.type, "picture");
  await e.page.evaluate(() => JM.dropping(true));
  assert.equal(await e.page.getAttribute("#text", "class"), "dropping");
  await e.page.evaluate(() => JM.dropping(false));
});

test("removing formats leaves the words", '<div id=a>ein <b>fettes</b> <font color="#d93025">rotes</font> <span style="font-size:15pt">großes</span> Wort</div>', async (e) => {
  await e.select("#a");
  await e.button("clear");
  assert.equal(await e.html(), '<div id="a">ein fettes rotes großes Wort</div>');
});

test("what is inserted goes where the cursor was, also while a menu's field has the keyboard", "<div id=a>eins</div><div>zwei</div>", async (e) => {
  await e.caret("#a");
  await e.press("Meta+k");
  await e.page.evaluate(() => JM.insert("<b>!</b>"));
  has(await e.html(), '<div id="a">eins<b>!</b></div>');
});

test("a draft from elsewhere cannot act on the page: what could is taken out, and that is saved", '<style>#bar{display:none}</style><div id=a style="position:fixed;top:0;color:red">Text <img name=execCommand alt=x><span style="display:none">verborgen</span></div><form><input name=body></form><div id=signature>meins</div>', async (e) => {
  assert.equal(await e.html(), '<div id="a" style="top: 0px; color: red;">Text <img alt="x" style="max-width: 100%; height: auto;"><span>verborgen</span></div><div id="signature">meins</div>');
  assert.equal(await e.page.isVisible("#bar"), true);
  // the page's own signature is still the page's
  assert.equal(await e.page.evaluate(() => !!document.querySelectorAll("#signature")[1].shadowRoot), true);
  await e.caret("#a");
  await e.press("Meta+b");
  await e.type("fett");
  has(await e.html(), "<b>fett</b>");
  // taken back, the text is what came, not less
  await e.press("Meta+z");
  await e.press("Meta+z");
  has(await e.html(), "<style>#bar{display:none}</style>");
});

test("the dark page keeps the author's colors and nothing else", '<div>ein <font color="#d93025" id=r>rotes</font> <span id=m style="background-color:#fff27a">markiertes</span> <span id=o style="color:#000;background:#fff">fremdes</span></div><table><tr><th id=h style="background-color:#f3f3f3">K</th></tr></table>', async (e) => {
  const color = (id, what) => e.page.evaluate(([id, what]) => getComputedStyle(document.getElementById(id))[what], [id, what]);
  assert.equal(await color("r", "color"), "rgb(217, 48, 37)");
  assert.equal(await color("m", "backgroundColor"), "rgb(255, 242, 122)");
  assert.equal(await color("m", "color"), "rgb(29, 29, 31)");
  assert.equal(await color("h", "backgroundColor"), "rgba(0, 0, 0, 0)");
  assert.notEqual(await color("o", "color"), "rgb(0, 0, 0)");
  // none of it reaches the mail, and editing leaves no colors of its own behind
  await e.caret("#m");
  await e.press("Enter");
  await e.type("- neu");
  const html = await e.html();
  hasNot(html, "class=");
  hasNot(html, "rgb(29, 29, 31)");
  await e.select("li");
  await e.button("color");
  await e.page.click("#pop .swatches:nth-of-type(4) .swatch:nth-child(3)");
  assert.equal(await e.page.evaluate(() => getComputedStyle(document.querySelector("#text li span")).color), "rgb(29, 29, 31)");
  hasNot(await e.html(), "class=");
}, { theme: "dark" });

const browser = await launch();
let failed = 0;
for (const { name, html, run, options } of tests) {
  if (only && !name.includes(only)) continue;
  const e = await editor(browser, html, options);
  try {
    await run(e);
    assert.deepEqual(e.errors, [], "the page's script failed");
    console.log(`ok    ${name}`);
  } catch (error) {
    failed++;
    console.log(`FAIL  ${name}\n${String(error.message).replace(/^/gm, "      ")}`);
  }
  await e.close();
}
await browser.close();
console.log(failed ? `${failed} failed` : "all passed");
process.exit(failed ? 1 : 0);
