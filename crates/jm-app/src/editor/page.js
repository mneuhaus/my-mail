// The draft editor's page (see ../editor.rs). `DATA` is what the app put into the page, `mount` shows a mail
// (the signature, the quote) in a shadow root. The app calls `JM.…`; the page reports through `send`.
(() => {
  const send = (m) => window.ipc.postMessage(JSON.stringify({ editor: Object.assign({ page: DATA.page }, m) }));
  // the page's own elements, before a draft's text is in the page: it may use the same ids
  const $ = (id) => document.getElementById(id);
  const text = $("text"), bar = $("bar"), pop = $("pop"), frame = $("frame"), main = $("main");
  const signature = $("signature"), quoted = $("quote"), quoteToggle = $("quote-toggle");
  const L = DATA.labels, STYLES = DATA.styles;
  const BLANK = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
  // What the script does to the text (as opposed to what is typed) goes through here.
  let scripted = 0;
  const exec = (name, value) => {
    scripted++;
    try {
      return document.execCommand(name, false, value);
    } finally {
      scripted--;
    }
  };
  // `el("button", { className: "item" }, "text", child)`
  const el = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children);
    return node;
  };
  const icon = (name) => el("span", { innerHTML: DATA.icons[name], style: "display:contents" });

  // ---- the text as the mail carries it and as the page shows it ---------------------------------------------

  // Pictures of the draft are `cid:` in the mail and show from `data:` URLs, by content id (lowercase).
  const known = Object.assign({}, DATA.pictures);
  // Mail HTML as the page shows it: pictures from their bytes, and nothing that could act on this page instead of
  // being text (a draft's HTML may come from anywhere): no styles, forms or frames, no names that stand for parts
  // of the page, nothing laid over the page or hidden in it. `cleaned` says that something was taken out.
  const ACTIVE = "script, style, link, meta, base, title, form, input, button, select, textarea, iframe, frame, object, embed, applet";
  const LAYOUT = ["position", "z-index", "pointer-events", "display", "visibility", "opacity"];
  let cleaned = false;
  const shown = (html) => {
    const parsed = el("template", { innerHTML: html });
    for (const active of parsed.content.querySelectorAll(ACTIVE)) {
      active.remove();
      cleaned = true;
    }
    for (const each of parsed.content.querySelectorAll("[name], [contenteditable], [style]")) {
      for (const name of ["name", "contenteditable"]) {
        cleaned ||= each.hasAttribute(name);
        each.removeAttribute(name);
      }
      for (const property of LAYOUT) {
        cleaned ||= !!each.style.getPropertyValue(property);
        each.style.removeProperty(property);
      }
      if (each.getAttribute("style") === "") each.removeAttribute("style");
    }
    for (const img of parsed.content.querySelectorAll("img")) {
      const src = img.getAttribute("src") || "";
      if (!/^cid:/i.test(src)) continue;
      const cid = src.slice(4), url = known[cid.toLowerCase()];
      img.dataset.cid = cid;
      img.setAttribute("src", url || BLANK);
      img.classList.toggle("loading", !url);
    }
    return parsed.innerHTML;
  };
  // The text as mail HTML, styled the way a mail needs it: mail programs know no stylesheet, and WebKit drops
  // from inserted HTML the styles this page's stylesheet gives anyway.
  const html = () => {
    const copy = text.cloneNode(true);
    for (const own of copy.querySelectorAll(".loading, .marked")) {
      own.classList.remove("loading", "marked");
      if (!own.classList.length) own.removeAttribute("class");
    }
    for (const img of copy.querySelectorAll("img")) {
      if (img.dataset.cid) img.setAttribute("src", "cid:" + img.dataset.cid);
      img.removeAttribute("data-cid");
      if (!img.style.maxWidth) Object.assign(img.style, { maxWidth: "100%", height: "auto" });
    }
    for (const list of copy.querySelectorAll("ul, ol")) {
      if (!list.style.marginTop) list.style.marginTop = "0";
      if (!list.style.marginBottom) list.style.marginBottom = "0";
    }
    for (const table of copy.querySelectorAll('table[cellspacing="0"]')) table.style.borderCollapse = "collapse";
    for (const quote of copy.querySelectorAll("blockquote:not([style])")) quote.setAttribute("style", STYLES.quote);
    return copy.innerHTML;
  };
  const empty = () => text.classList.toggle("empty", !text.textContent.trim() && !text.querySelector("img, table, li, hr"));
  // In dark mode marked text keeps its light color and gets dark letters (see page.css): by a class of the page.
  const dark = document.documentElement.classList.contains("dark");
  const tagMarked = () => {
    if (!dark) return;
    for (const each of text.querySelectorAll("[style*='background'], .marked")) {
      const color = each.style.backgroundColor;
      each.classList.toggle("marked", !!color && color !== "transparent" && getComputedStyle(each).display.startsWith("inline"));
    }
  };

  // ---- the selection ---------------------------------------------------------------------------------------

  const selection = () => getSelection();
  const inText = (node) => !!node && text.contains(node);
  // The element matching `selector` the whole selection lies in.
  const closest = (selector) => {
    const s = selection();
    let node = s.rangeCount ? s.getRangeAt(0).commonAncestorContainer : null;
    if (node && node.nodeType !== Node.ELEMENT_NODE) node = node.parentElement;
    const found = node && node.closest(selector);
    return found && found !== text && text.contains(found) ? found : null;
  };
  const caretIn = (node, atEnd = true) => {
    const s = selection();
    s.selectAllChildren(node);
    if (atEnd) s.collapseToEnd();
    else s.collapseToStart();
  };
  // Where the selection is, as the way down from the text to its two ends: it survives the text being set again.
  const pathTo = (node, offset) => {
    const path = [offset];
    for (let n = node; n !== text; n = n.parentNode) path.unshift([...n.parentNode.childNodes].indexOf(n));
    return path;
  };
  const pointAt = (path) => {
    let node = text;
    for (const index of path.slice(0, -1)) {
      if (!node.childNodes[index]) return [node, node.childNodes.length];
      node = node.childNodes[index];
    }
    const length = node.nodeType === Node.TEXT_NODE ? node.length : node.childNodes.length;
    return [node, Math.min(path.at(-1), length)];
  };
  const where = () => {
    const s = selection();
    if (!s.rangeCount || !inText(s.anchorNode) || !inText(s.focusNode)) return null;
    return [pathTo(s.anchorNode, s.anchorOffset), pathTo(s.focusNode, s.focusOffset)];
  };
  // The line the cursor is in: what stands before the cursor (a range) and after it (text).
  const line = () => {
    const s = selection(), caret = s.getRangeAt(0).cloneRange();
    const back = () => {
      s.removeAllRanges();
      s.addRange(caret);
    };
    s.modify("extend", "backward", "paragraphboundary");
    const before = s.getRangeAt(0).cloneRange();
    back();
    s.modify("extend", "forward", "paragraphboundary");
    const after = s.toString();
    back();
    return { before, after };
  };

  // ---- undo -------------------------------------------------------------------------------------------------

  // The text as it was, change by change. WebKit's own undo only knows what was typed or done through
  // `execCommand`; it loses track when the script changes the text (a table row, the size of a picture).
  const history = { states: [], at: -1, kind: "", time: 0 };
  const TYPING = new Set(["insertText", "deleteContentBackward", "deleteContentForward"]);
  let quiet = 0;
  // After every change: remember it and tell the app. `kind` is the input type; typing in one go is one step.
  const changed = (kind = "") => {
    if (quiet) return;
    empty();
    tagMarked();
    text.normalize(); // as the HTML parses again, so `where` fits after undo
    const state = { html: html(), where: where() }, now = Date.now(), top = history.states[history.at];
    if (top && top.html === state.html) return;
    const typing = history.at > 0 && TYPING.has(kind) && kind === history.kind && now - history.time < 1000;
    history.states.length = history.at + (typing ? 0 : 1);
    history.states.push(state);
    // not without end: 200 steps, or less of a very long text
    const size = () => history.states.reduce((sum, each) => sum + each.html.length, 0);
    while (history.states.length > 200 || (history.states.length > 2 && size() > 8e6)) history.states.shift();
    Object.assign(history, { at: history.states.length - 1, kind, time: now });
    send({ type: "edited", html: state.html });
  };
  // Several changes as one step, apart from what was typed before.
  const together = (change) => {
    history.kind = "";
    quiet++;
    try {
      change();
    } finally {
      quiet--;
    }
    changed();
  };
  const travel = (steps) => {
    const state = history.states[history.at + steps];
    if (!state) return;
    Object.assign(history, { at: history.at + steps, kind: "" });
    text.innerHTML = shown(state.html);
    text.focus();
    if (state.where) selection().setBaseAndExtent(...pointAt(state.where[0]), ...pointAt(state.where[1]));
    else caretIn(text);
    empty();
    tagMarked();
    unframe();
    send({ type: "edited", html: state.html });
  };

  text.innerHTML = shown(DATA.html);
  empty();
  tagMarked();
  exec("defaultParagraphSeparator", "div");
  // the draft as it came is where undo ends; what was taken out of it is a change to save
  history.states.push({ html: cleaned ? DATA.html : html(), where: null });
  history.at = 0;
  if (cleaned) changed();
  text.addEventListener("input", (e) => e.isComposing || changed(scripted ? "" : e.inputType));
  text.addEventListener("compositionend", () => changed("insertText"));
  text.addEventListener("beforeinput", (e) => {
    if (e.inputType !== "historyUndo" && e.inputType !== "historyRedo") return;
    e.preventDefault();
    travel(e.inputType === "historyUndo" ? -1 : 1);
  });

  // Where the cursor was, for what the bar and the app do while something else has the keyboard.
  let saved = null, refreshing = false;
  document.addEventListener("selectionchange", () => {
    const s = selection();
    if (s.rangeCount && inText(s.getRangeAt(0).commonAncestorContainer)) {
      saved = s.getRangeAt(0).cloneRange();
      if (!quiet && history.states[history.at]) history.states[history.at].where = where();
    }
    if (picked && !isSelected(picked)) unframe();
    if (refreshing) return;
    refreshing = true;
    requestAnimationFrame(() => {
      refreshing = false;
      refresh();
    });
  });
  const restore = () => {
    if (document.activeElement !== text) text.focus();
    if (saved) {
      const s = selection();
      s.removeAllRanges();
      s.addRange(saved);
    }
  };
  const focused = () => send({ type: "focused" });
  text.addEventListener("focus", focused);
  window.addEventListener("focus", () => document.activeElement === text && focused());

  // ---- size, color, quote, link ----------------------------------------------------------------------------

  // An element that carries nothing anymore gives way to what it holds.
  const unwrap = (node) => node.replaceWith(...node.childNodes);
  const bare = (node) => {
    if (node.getAttribute("style") === "") node.removeAttribute("style");
    if (node.matches("span, font") && !node.attributes.length) unwrap(node);
  };
  // For a size or a color WebKit wraps exactly what is selected in <font> (splitting what was there); `fix` makes
  // of each what the mail should carry. The selection stays on what was changed.
  const rewrap = (command, mark, selector, fix) => {
    exec(command, mark);
    const made = [...text.querySelectorAll(selector)];
    if (!made.length) return;
    const start = document.createComment(""), end = document.createComment(""), range = document.createRange();
    made[0].before(start);
    made.at(-1).after(end);
    made.forEach(fix);
    range.setStartAfter(start);
    range.setEndBefore(end);
    start.remove();
    end.remove();
    selection().removeAllRanges();
    selection().addRange(range);
  };

  // As Markdown headings from `jm` come out: the text of a mail is 11pt.
  const SIZES = { small: "9pt", normal: "", large: "13pt", huge: "15pt" };
  const currentSize = () => {
    const s = selection();
    for (let node = s.rangeCount ? s.getRangeAt(0).startContainer : null; node && node !== text; node = node.parentNode) {
      const value = node.style && node.style.fontSize;
      if (value) return Object.keys(SIZES).find((name) => SIZES[name] === value) || "";
    }
    return "normal";
  };
  // Of what is selected, or of the line the cursor is in (the cursor stays where it is).
  const size = (name) => together(() => {
    const s = selection(), onLine = s.isCollapsed;
    const typed = onLine ? [...new Intl.Segmenter().segment(line().before.toString())].length : 0;
    if (onLine) {
      s.modify("move", "backward", "paragraphboundary");
      s.modify("extend", "forward", "paragraphboundary");
    }
    if (s.isCollapsed) return; // an empty line
    rewrap("fontSize", 7, 'font[size="7"]', (font) => {
      for (const inner of font.querySelectorAll("[style*='font-size'], font[size]")) {
        inner.style.fontSize = "";
        inner.removeAttribute("size");
        bare(inner);
      }
      if (SIZES[name]) font.replaceWith(el("span", { style: `font-size:${SIZES[name]}` }, ...font.childNodes));
      else unwrap(font);
    });
    if (!onLine) return;
    s.collapseToStart();
    for (let i = 0; i < typed; i++) s.modify("move", "forward", "character");
  });

  const COLORS = ["#d93025", "#e8710a", "#188038", "#1a73e8", "#8430ce", "#80868b"];
  const MARKS = ["#fff27a", "#c8f7c5", "#cfe8ff", "#ffd6e7", "#ffe0b2"];
  // The color of the text; without a `value` it is the text's own again.
  const color = (value) => {
    if (value) return exec("foreColor", value); // without a selection: of what is typed next
    if (selection().isCollapsed) return;
    together(() => rewrap("foreColor", "#010203", 'font[color="#010203"]', (font) => {
      for (const inner of font.querySelectorAll("font[color], [style*='color']")) {
        inner.removeAttribute("color");
        inner.style.color = "";
        bare(inner);
      }
      unwrap(font);
    }));
  };
  const mark = (value) => exec("hiliteColor", value || "transparent");

  // The quotes the selection touches.
  const quotes = () => [...text.querySelectorAll("blockquote")].filter((each) => selection().containsNode(each, true));
  // The lines selected become a quote, or lines of the text again if they are one.
  const quote = () => together(() => {
    const found = quotes(), s = selection();
    if (!found.length) return exec("formatBlock", "blockquote");
    const ends = [s.anchorNode, s.anchorOffset, s.focusNode, s.focusOffset];
    for (const each of found) {
      // lines of their own: they are blocks already, or become one
      const block = each.querySelector("div, p, ul, ol, table, blockquote, pre") ? null : el("div", {}, ...each.childNodes);
      for (const end of [0, 2]) if (ends[end] === each && block) ends[end] = block;
      if (block) each.replaceWith(block);
      else unwrap(each);
    }
    if (ends[0].isConnected && ends[2].isConnected) s.setBaseAndExtent(...ends);
  });

  // What was typed into the link's field as an address a mail may carry ("" if it is none).
  const address = (typed) => {
    const value = typed.trim();
    if (!value || /^(https?:\/\/|mailto:|tel:)/i.test(value)) return value;
    if (/^[^\s@/]+@[^\s@/]+\.[^\s@/]+$/.test(value)) return "mailto:" + value;
    return /^[a-z][a-z0-9+-]*:/i.test(value) ? "" : "https://" + value;
  };
  // Link what is selected (or the link the cursor is in) to `typed`; nothing typed takes the link away.
  const link = (typed) => {
    const url = address(typed), a = closest("a[href]");
    if (a && url) {
      a.setAttribute("href", url);
      changed();
    } else if (a) {
      selection().selectAllChildren(a);
      exec("unlink");
    } else if (url && selection().isCollapsed) {
      exec("insertHTML", el("a", { href: url }, typed.trim()).outerHTML);
    } else if (url) {
      exec("createLink", url);
    }
  };
  const ADDRESS = /((?:https?:\/\/|www\.)[^\s<>"']*[^\s<>"'.,;:!?)\]])([.,;:!?)\]]*)$/i;
  // A web address just typed becomes a link once a space or a new line ends it.
  const autolink = () => {
    const s = selection(), node = s.anchorNode;
    if (!s.isCollapsed || node.nodeType !== Node.TEXT_NODE || closest("a")) return;
    const found = node.data.slice(0, s.anchorOffset).match(ADDRESS);
    if (!found) return;
    const end = s.anchorOffset - found[2].length, range = document.createRange(), a = el("a", { href: address(found[1]) });
    range.setStart(node, end - found[1].length);
    range.setEnd(node, end);
    range.surroundContents(a);
    // the cursor goes on behind it
    if (a.nextSibling && a.nextSibling.nodeType === Node.TEXT_NODE) s.collapse(a.nextSibling, found[2].length);
    else s.collapse(a.parentNode, [...a.parentNode.childNodes].indexOf(a) + 1);
  };
  // "- " or "1. " at the start of a line begins a list.
  const autolist = () => {
    if (!selection().isCollapsed || closest("li, pre")) return false;
    const { before } = line(), typed = before.toString();
    const command = /^[-*]$/.test(typed) ? "insertUnorderedList" : /^1[.)]$/.test(typed) ? "insertOrderedList" : "";
    if (!command) return false;
    together(() => {
      exec(command);
      const item = closest("li"), marker = item && document.createTreeWalker(item, NodeFilter.SHOW_TEXT).nextNode();
      if (!marker) return;
      marker.deleteData(0, typed.length);
      if (!item.textContent && !item.querySelector("br")) item.append(el("br"));
      caretIn(item, false);
    });
    return true;
  };

  // ---- tables -----------------------------------------------------------------------------------------------

  // A new table where the cursor is, its first row the header.
  const table = (columns, rows) => together(() => {
    const row = (tag) => `<tr>${`<${tag}><br></${tag}>`.repeat(columns)}</tr>`;
    exec("insertHTML", `<table data-new cellpadding="0" cellspacing="0">${row("th")}${row("td").repeat(rows - 1)}</table>`);
    const made = text.querySelector("table[data-new]");
    if (!made) return;
    made.removeAttribute("data-new");
    // styled here: WebKit drops styles from what it inserts
    for (const cell of made.querySelectorAll("th, td")) cell.setAttribute("style", `${STYLES[cell.localName]};text-align:left`);
    if (!made.nextSibling) made.after(el("div", {}, el("br"))); // a line to go on writing below it
    caretIn(made.rows[0].cells[0]);
  });
  // An empty cell for the column of `like`.
  const blank = (like, tag = like.localName) => {
    const cell = el(tag, {}, el("br"));
    cell.setAttribute("style", (like.matches(tag) && like.getAttribute("style")) || STYLES[tag]);
    if (like.style.textAlign) cell.style.textAlign = like.style.textAlign;
    return cell;
  };
  const addRow = (cell, side) => {
    const row = cell.parentElement, fresh = el("tr", {}, ...[...row.cells].map((c) => blank(c, "td")));
    const body = row.closest("table").tBodies[0];
    // below the header: the first row of the body
    if (side === "after" && row.parentElement.matches("thead") && body) body.prepend(fresh);
    else row[side](fresh);
    caretIn(fresh.cells[cell.cellIndex]);
    return fresh;
  };
  const addColumn = (cell, side) => {
    const index = cell.cellIndex;
    for (const row of cell.closest("table").rows) {
      const like = row.cells[Math.min(index, row.cells.length - 1)];
      if (like) like[side](blank(like));
    }
    caretIn(cell.parentElement.cells[index + (side === "after" ? 1 : 0)]);
  };
  const removeTable = (table) => {
    const next = table.nextElementSibling;
    table.remove();
    if (!text.firstChild) text.append(el("br"));
    caretIn(next || text, !next);
  };
  const removeRow = (cell) => {
    const table = cell.closest("table"), row = cell.parentElement;
    if (table.rows.length === 1) return removeTable(table);
    const next = table.rows[row.rowIndex + 1] || table.rows[row.rowIndex - 1];
    row.remove();
    caretIn(next.cells[Math.min(cell.cellIndex, next.cells.length - 1)]);
  };
  const removeColumn = (cell) => {
    const table = cell.closest("table"), row = cell.parentElement, index = cell.cellIndex;
    if (row.cells.length === 1) return removeTable(table);
    for (const each of table.rows) if (each.cells[index]) each.cells[index].remove();
    caretIn(row.cells[Math.min(index, row.cells.length - 1)]);
  };
  // ⇥ in a table: on to the next cell (a new row after the last one), ⇧⇥ back.
  const nextCell = (cell, back) => {
    const cells = [...cell.closest("table").rows].flatMap((row) => [...row.cells]);
    const next = cells[cells.indexOf(cell) + (back ? -1 : 1)];
    if (next) caretIn(next);
    else if (!back) together(() => caretIn(addRow(cell, "after").cells[0]));
  };

  // ---- pictures ---------------------------------------------------------------------------------------------

  // The picture that was clicked has a frame; dragging its corner changes the size.
  let picked = null;
  const isSelected = (node) => {
    const s = selection(), range = s.rangeCount && s.getRangeAt(0);
    return !!range && range.startContainer === node.parentNode && range.endContainer === node.parentNode
      && range.startContainer.childNodes[range.startOffset] === node && range.endOffset === range.startOffset + 1;
  };
  const place = () => {
    if (!picked) return;
    const box = picked.getBoundingClientRect();
    Object.assign(frame.style, {
      left: `${box.left + scrollX}px`, top: `${box.top + scrollY}px`, width: `${box.width}px`, height: `${box.height}px`,
    });
    frame.lastElementChild.textContent = `${Math.round(box.width)} × ${Math.round(box.height)}`;
  };
  const unframe = () => {
    picked = null;
    frame.hidden = true;
  };
  const pick = (img) => {
    const range = document.createRange();
    range.selectNode(img);
    selection().removeAllRanges();
    selection().addRange(range);
    picked = img;
    frame.hidden = false;
    place();
  };
  text.addEventListener("click", (e) => {
    if (e.target.tagName === "IMG") return pick(e.target);
    unframe();
    // ⌘ and a click open a link, as everywhere on a Mac
    const a = e.metaKey && e.target.closest("a[href]");
    if (a) window.ipc.postMessage(JSON.stringify({ open: a.href }));
  });
  window.addEventListener("resize", place);
  frame.firstElementChild.addEventListener("pointerdown", (e) => {
    if (!picked) return;
    e.preventDefault();
    const handle = e.currentTarget, img = picked, box = img.getBoundingClientRect();
    const drag = (move) => {
      const width = Math.round(Math.max(24, Math.min(text.clientWidth, box.width + move.clientX - e.clientX)));
      img.setAttribute("width", width);
      img.setAttribute("height", Math.max(1, Math.round((width * box.height) / box.width)));
      place();
    };
    handle.setPointerCapture(e.pointerId);
    handle.addEventListener("pointermove", drag);
    // let go, or taken away
    handle.addEventListener("lostpointercapture", () => {
      handle.removeEventListener("pointermove", drag);
      changed();
    }, { once: true });
  });

  // ---- the bar and what its buttons open --------------------------------------------------------------------

  const commands = {
    bold: () => exec("bold"),
    italic: () => exec("italic"),
    underline: () => exec("underline"),
    strike: () => exec("strikeThrough"),
    bullets: () => exec("insertUnorderedList"),
    numbers: () => exec("insertOrderedList"),
    quote,
    picture: () => send({ type: "picture" }),
    clear: () => exec("removeFormat"),
  };
  const states = {
    bold: () => document.queryCommandState("bold"),
    italic: () => document.queryCommandState("italic"),
    underline: () => document.queryCommandState("underline"),
    strike: () => document.queryCommandState("strikeThrough"),
    bullets: () => document.queryCommandState("insertUnorderedList"),
    numbers: () => document.queryCommandState("insertOrderedList"),
    quote: () => quotes().length,
    link: () => closest("a[href]"),
    table: () => closest("td, th"),
  };
  // The buttons show what holds where the cursor is.
  const refresh = () => {
    const active = inText(selection().anchorNode);
    for (const button of bar.querySelectorAll("button")) {
      const state = states[button.dataset.cmd || button.dataset.menu];
      if (state) button.classList.toggle("on", active && !!state());
    }
  };

  let opened = "";
  const close = () => {
    if (!opened) return;
    bar.querySelector(".open").classList.remove("open");
    pop.hidden = true;
    pop.replaceChildren();
    opened = "";
  };
  // What an entry of a menu does: its thing on the selection in the text, and the menu closes.
  const act = (action) => {
    close();
    restore();
    history.kind = ""; // what is typed after it is a step of its own
    action();
    refresh();
  };
  const item = (label, action, { checked = null, danger = false } = {}) => {
    const node = el("button", { type: "button", className: danger ? "item danger" : "item" }, label);
    if (checked !== null) node.prepend(el("span", { className: "check" }, checked ? icon("check") : ""));
    node.addEventListener("click", () => act(action));
    return node;
  };
  const tool = (name, label, action) => {
    const node = el("button", { type: "button", className: "icon", title: label }, icon(name));
    node.addEventListener("click", action);
    return node;
  };
  const swatches = (values, apply, none) => el("div", { className: "swatches" }, ...values.map((value) => {
    const node = el("button", { type: "button", className: "swatch", title: value ? "" : none }, value ? "" : icon("none"));
    node.style.background = value;
    node.addEventListener("click", () => act(() => apply(value)));
    return node;
  }));
  const menus = {
    size: () => Object.keys(SIZES).map((name) => {
      const node = item(L[name], () => size(name), { checked: currentSize() === name });
      node.style.fontSize = SIZES[name] || "11pt";
      return node;
    }),
    color: () => [
      el("div", { className: "label" }, L.color),
      swatches(["", ...COLORS], color, L.automatic),
      el("div", { className: "label" }, L.mark),
      swatches(["", ...MARKS], mark, L.unmarked),
    ],
    link: () => {
      const a = closest("a[href]"), selected = selection().toString().trim();
      const guess = /^((https?:\/\/|www\.)\S+|[^\s@]+@[^\s@]+\.[^\s@]+)$/i.test(selected) ? selected : "";
      const input = el("input", { type: "text", placeholder: L.address, value: a ? a.getAttribute("href") : guess, spellcheck: false });
      const apply = () => act(() => link(input.value));
      input.addEventListener("keydown", (e) => {
        if (e.key !== "Enter") return;
        e.preventDefault();
        apply();
      });
      const row = el("div", { className: "link" }, input, tool("check", L.apply, apply));
      if (a) {
        row.append(
          tool("open", L.open, () => {
            close();
            window.ipc.postMessage(JSON.stringify({ open: a.href }));
          }),
          tool("unlink", L.unlink, () => act(() => link(""))),
        );
      }
      setTimeout(() => input.select()); // once the menu shows
      return [row];
    },
    table: () => {
      const cell = closest("td, th");
      if (cell) {
        const change = (label, what, options) => item(label, () => together(() => what(cell)), options);
        return [
          change(L.rowAbove, (c) => addRow(c, "before")),
          change(L.rowBelow, (c) => addRow(c, "after")),
          change(L.columnLeft, (c) => addColumn(c, "before")),
          change(L.columnRight, (c) => addColumn(c, "after")),
          el("hr"),
          change(L.removeRow, removeRow),
          change(L.removeColumn, removeColumn),
          change(L.removeTable, (c) => removeTable(c.closest("table")), { danger: true }),
        ];
      }
      // boxes to pick the size from, six columns wide
      const hint = el("div", { className: "hint" }, L.table), grid = el("div", { className: "grid" });
      for (let i = 0; i < 30; i++) grid.append(el("i"));
      const sizeAt = (box) => {
        const index = [...grid.children].indexOf(box);
        return index < 0 ? null : [(index % 6) + 1, Math.floor(index / 6) + 1];
      };
      grid.addEventListener("mouseover", (e) => {
        const [columns, rows] = sizeAt(e.target) || [];
        if (!columns) return;
        [...grid.children].forEach((box, i) => box.classList.toggle("on", i % 6 < columns && Math.floor(i / 6) < rows));
        hint.textContent = `${columns} × ${rows}`;
      });
      grid.addEventListener("click", (e) => {
        const chosen = sizeAt(e.target);
        if (chosen) act(() => table(...chosen));
      });
      return [grid, hint];
    },
  };
  const open = (name) => {
    const button = bar.querySelector(`[data-menu="${name}"]`), again = opened === name;
    close();
    if (again) return;
    refresh();
    pop.append(...menus[name]());
    pop.hidden = false;
    const box = button.getBoundingClientRect();
    pop.style.top = `${box.bottom + 4}px`;
    pop.style.left = `${Math.max(8, Math.min(box.left, innerWidth - pop.offsetWidth - 8))}px`;
    button.classList.add("open");
    opened = name;
  };
  // a click on the bar or a menu leaves the cursor where it is
  bar.addEventListener("mousedown", (e) => e.preventDefault());
  pop.addEventListener("mousedown", (e) => e.target.tagName === "INPUT" || e.preventDefault());
  bar.addEventListener("click", (e) => {
    const button = e.target.closest("button");
    if (!button) return;
    if (button.dataset.menu) return open(button.dataset.menu);
    act(commands[button.dataset.cmd]);
  });
  window.addEventListener("blur", () => close());
  document.addEventListener("mousedown", (e) => {
    if (opened && !pop.contains(e.target) && !bar.contains(e.target)) close();
    // a click below the text puts the cursor at its end
    if (![document.documentElement, document.body, main].includes(e.target)) return;
    e.preventDefault();
    text.focus();
    caretIn(text);
  }, true);

  // ---- keys -------------------------------------------------------------------------------------------------

  const withCommand = { b: "bold", i: "italic", u: "underline" };
  const moving = new Set(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown"]);
  document.addEventListener("keydown", (e) => {
    if (e.isComposing) return;
    if (e.key === "Escape" && (opened || picked)) {
      e.preventDefault();
      close();
      unframe();
      return restore();
    }
    if (pop.contains(e.target)) return; // the field of a menu takes its own keys
    const key = e.key.toLowerCase(), done = () => e.preventDefault();
    if (e.metaKey && !e.ctrlKey) {
      history.kind = ""; // what is typed after a command is a step of its own
      if (key === "v") {
        // ⇧⌘V and ⌥⇧⌘V: as text, without what it looked like
        done();
        send({ type: "paste", plain: e.shiftKey });
      } else if (e.altKey) {
        return;
      } else if (key === "z") {
        done();
        travel(e.shiftKey ? 1 : -1);
      } else if (e.shiftKey) {
        const command = e.code === "Digit7" ? "numbers" : e.code === "Digit8" ? "bullets" : key === "x" ? "strike" : "";
        if (!command) return;
        done();
        act(commands[command]);
      } else if (withCommand[key]) {
        done();
        act(commands[withCommand[key]]);
      } else if (key === "k") {
        done();
        open("link");
      } else if (key === "a") {
        done();
        selection().selectAllChildren(inText(selection().anchorNode) ? text : document.body);
      } else if (key === "x" || key === "c") {
        done();
        exec(key === "x" ? "cut" : "copy");
      }
      return;
    }
    if (e.ctrlKey || e.altKey || !inText(selection().anchorNode)) return;
    if (moving.has(e.key)) {
      history.kind = ""; // what is typed after moving the cursor is a step of its own
    } else if (e.key === "Tab") {
      done();
      const cell = closest("td, th");
      if (cell) nextCell(cell, e.shiftKey);
      else if (closest("li")) exec(e.shiftKey ? "outdent" : "indent");
      else if (e.shiftKey) send({ type: "leave" });
    } else if (e.key === " " && !e.shiftKey) {
      if (autolist()) done();
      else autolink();
    } else if (e.key === "Enter" && !e.shiftKey) {
      const { before, after } = selection().isCollapsed ? line() : {};
      // ↩ on an empty line of a quote ends the quote
      if (before && !before.toString() && !after && quotes().length && !closest("li")) {
        done();
        quote();
      } else {
        autolink();
      }
    }
  }, true);
  text.addEventListener("paste", (e) => {
    e.preventDefault();
    send({ type: "paste", plain: false });
  });
  // files come in through the app (it knows their paths); dropped here, WebKit would try to open them
  for (const type of ["dragover", "drop"]) {
    document.addEventListener(type, (e) => [...(e.dataTransfer?.types || [])].includes("Files") && e.preventDefault());
  }

  // ---- below the text: the signature and a reply's quote, as they go out ------------------------------------

  if (DATA.signature) {
    mount(signature, DATA.signature);
    signature.hidden = false;
  }
  if (DATA.quote) {
    quoteToggle.textContent = L.showQuote;
    quoteToggle.hidden = false;
    quoteToggle.addEventListener("click", () => {
      if (!quoted.shadowRoot) mount(quoted, DATA.quote);
      quoted.hidden = !quoted.hidden;
      quoteToggle.textContent = quoted.hidden ? L.showQuote : L.hideQuote;
    });
  }
  // WebKit's menu (spelling, look up, copy) in the text only: around it, it offers "Reload"
  document.addEventListener("contextmenu", (e) => inText(e.target) || pop.contains(e.target) || e.preventDefault());

  // ---- what the app asks for ---------------------------------------------------------------------------------

  window.JM = {
    // mail HTML where the cursor is
    insert(markup) {
      restore();
      exec("insertHTML", shown(markup));
    },
    // pictures whose bytes came, as `data:` URLs by content id
    pictures(found) {
      Object.assign(known, found);
      for (const img of text.querySelectorAll("img.loading[data-cid]")) {
        const url = known[img.dataset.cid.toLowerCase()];
        if (!url) continue;
        img.src = url;
        img.classList.remove("loading");
      }
    },
    // a picture that could not go into the draft
    drop(cid) {
      for (const img of text.querySelectorAll("img[data-cid]")) if (img.dataset.cid === cid) img.remove();
      changed();
    },
    // a web address was pasted: what is selected links to it, else it stands there as a link
    link(url) {
      restore();
      if (selection().isCollapsed && closest("a[href]")) exec("insertText", url);
      else link(url);
    },
    // files are dragged over the text (`on`), and where they are let go the cursor goes
    dropping(on) {
      text.classList.toggle("dropping", on);
    },
    caret(x, y) {
      const range = document.caretRangeFromPoint(x, y);
      if (!range || !inText(range.startContainer)) return;
      saved = range;
      selection().removeAllRanges();
      selection().addRange(range);
    },
    focus() {
      text.focus();
      if (!saved) caretIn(text, false);
    },
    // every change was sent right away; this one comes after them
    flush: () => send({ type: "flushed" }),
  };
  if (DATA.focus) JM.focus();
  refresh();
  send({ type: "ready" });
})();
