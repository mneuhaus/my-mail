// A tiny stand-in for Microsoft Graph with made-up mail, for screenshots and demos.
// Usage: node tools/demo/server.mjs [port]   (tools/demo/run.sh starts it with the app)
//
// Implements just what Just Mail and jm call: folders, the well-known batch, listing with the
// filters they use, search, conversations, messages with bodies, attachments, and draft/flag/read/move
// changes.
// The bearer token picks the mailbox ("demo-alex" or "demo-billing"). Nothing leaves localhost.
import http from "node:http";
import { readFileSync } from "node:fs";

const port = Number(process.argv[2] || 7357);
const base = `http://127.0.0.1:${port}/v1.0`;
const now = Date.now();
const ago = (minutes) => new Date(now - minutes * 60_000).toISOString().replace(/\.\d+Z$/, "Z");
const person = (name, address) => ({ emailAddress: { name, address } });
const me = person("Alex Morgan", "alex@northwind.example");

const folder = (id, displayName, extra = {}) => ({
  id, displayName, parentFolderId: "root", childFolderCount: 0, unreadItemCount: 0, totalItemCount: 0, ...extra,
});

let nextId = 1;
// an attachment as Graph keeps it after an upload (bytes still base64 in contentBytes)
const uploaded = (a) => ({ ...a, id: `att-${nextId++}`, size: a.contentBytes ? Buffer.from(a.contentBytes, "base64").length : 0 });
const message = (folderId, minutes, from, subject, html, extra = {}) => ({
  id: `msg-${nextId++}`,
  subject,
  bodyPreview: html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim().slice(0, 180),
  isRead: true,
  isDraft: false,
  hasAttachments: false,
  importance: "normal",
  receivedDateTime: ago(minutes),
  sentDateTime: ago(minutes),
  lastModifiedDateTime: ago(minutes),
  conversationId: `conv-${nextId}`,
  parentFolderId: folderId,
  webLink: "https://outlook.office.com/",
  flag: { flagStatus: "notFlagged" },
  from,
  toRecipients: [me],
  ccRecipients: [],
  bccRecipients: [],
  body: { contentType: "html", content: `<html><body>${html}</body></html>` },
  attachments: [],
  ...extra,
});
// Tom's Outlook signature: a logo embedded as an inline attachment (cid:) and a label/value table
const logo = readFileSync(new URL("./becker-logo.png", import.meta.url));
const beckerSignature = `<table><tr><td><p><img width="200" height="40" src="cid:image001.png@becker" alt="Becker &amp; Sons"></p></td></tr>
<tr><td><p><b>Tom Becker</b><br>Project lead</p></td></tr>
<tr><td><table><tr><td><p><b>Phone</b></p></td><td><p>+1 555 0100</p></td></tr>
<tr><td><p><b>Web</b></p></td><td><p><a href="https://beckerandsons.example">beckerandsons.example</a></p></td></tr></table></td></tr></table>`;
const pdf = (name, size) => ({
  "@odata.type": "#microsoft.graph.fileAttachment", id: `att-${name}`, name, contentType: "application/pdf", size, isInline: false,
});

const lines = (...l) => l.map((x) => `<div>${x || "<br>"}</div>`).join("");

const alex = {
  folders: [
    folder("f-inbox", "Inbox", { childFolderCount: 2 }),
    folder("f-drafts", "Drafts"),
    folder("f-sent", "Sent Items"),
    folder("f-archive", "Archive"),
    folder("f-junk", "Junk Email"),
    folder("f-trash", "Deleted Items"),
    folder("f-clients", "Clients", { parentFolderId: "f-inbox" }),
    folder("f-receipts", "Receipts", { parentFolderId: "f-inbox" }),
    folder("f-newsletters", "Newsletters"),
    folder("f-projects", "Projects"),
  ],
  wellKnown: { inbox: "f-inbox", drafts: "f-drafts", sentitems: "f-sent", archive: "f-archive", junkemail: "f-junk", deleteditems: "f-trash" },
  messages: [
    message("f-inbox", 26, person("Lena Hoffmann", "lena@harbor-labs.example"), "Kickoff notes for the Harbor project",
      lines("Hi Alex,", "", "attached are the notes from this morning. The short version: we start with the booking flow, design reviews every Tuesday, first prototype by the 24th.", "", "Could you check the timeline on page 2?", "", "Thanks, Lena"),
      { flag: { flagStatus: "flagged" }, hasAttachments: true, attachments: [pdf("harbor-kickoff-notes.pdf", 284_000)], isRead: false }),
    message("f-inbox", 47, person("Tom Becker", "tom@beckerandsons.example"), "Re: Quote for the website relaunch",
      `<div>Hi Alex,</div><div><br></div><div>thanks for the quick turnaround. The quote looks good to us, two things before we sign:</div><ul><li><b>Hosting:</b> can the first year be part of the package?</li><li><b>Content migration:</b> we have about 40 product pages, is that covered?</li></ul><div>If both are fine, we would like to start on <b>November 3</b>. The signed order form follows as soon as you confirm.</div><div><br></div><div>Our brand guide is here: <a href="https://beckerandsons.example/brand">beckerandsons.example/brand</a></div><div><br></div><div>Best regards</div>${beckerSignature}`,
      { flag: { flagStatus: "flagged" }, hasAttachments: true, conversationId: "conv-quote",
        attachments: [pdf("quote-relaunch-v2.pdf", 142_000), { "@odata.type": "#microsoft.graph.fileAttachment", id: "att-logo",
          name: "image001.png", contentType: "image/png", size: logo.length, isInline: true, contentId: "image001.png@becker", bytes: logo }] }),
    message("f-inbox", 95, person("Maria Lopez", "maria@lopez.example"), "Lunch on Thursday?",
      lines("Hey Alex,", "", "are you around on Thursday? The new place next to the station opened, I would love to try it.", "", "Maria"), { isRead: false }),
    message("f-inbox", 180, person("Paperclip Print Shop", "orders@paperclip.example"), "Your order #4821 has shipped",
      lines("Good news: your business cards are on their way.", "Tracking number: 00340434292135100125", "", "Paperclip Print Shop"),
      { isRead: false, hasAttachments: true, attachments: [pdf("invoice-4821.pdf", 58_000)] }),
    message("f-inbox", 60 * 20, person("Sam Carter", "sam@carter.example"), "Feedback on the prototype",
      lines("Hi Alex,", "", "played with the prototype over the weekend. The onboarding is great, the settings page felt crowded. Notes below.", "", "Sam"),
      { conversationId: "conv-prototype" }),
    message("f-inbox", 60 * 26, person("Calendar", "calendar@northwind.example"), "Team sync moved to 3 pm",
      lines("The weekly team sync on Wednesday now starts at 3 pm.")),
    message("f-inbox", 60 * 30, person("Jonas Weber", "jonas@weber-studio.example"), "Invoice September",
      lines("Hi Alex,", "", "please find my invoice for September attached.", "", "Jonas"),
      { hasAttachments: true, attachments: [pdf("invoice-2026-09.pdf", 96_000)] }),
    message("f-inbox", 60 * 50, person("Anna Schmidt", "anna@schmidt.example"), "Workshop slides",
      lines("Here are the slides from Friday, including the exercises.", "", "Anna"),
      { hasAttachments: true, attachments: [pdf("workshop-slides.pdf", 1_820_000)] }),
    message("f-inbox", 60 * 75, person("Northwind Bank", "service@bank.example"), "Your monthly statement is ready",
      lines("Your statement for September is available in online banking.")),
    message("f-inbox", 60 * 100, person("Laura Klein", "laura@klein.example"), "Photos from the offsite",
      lines("A few favourites from the offsite, the full album follows.", "", "Laura")),
    message("f-inbox", 60 * 140, person("Hosting Co.", "status@hosting.example"), "Maintenance window on Sunday",
      lines("Planned maintenance on Sunday, 02:00 to 04:00 UTC. No action needed.")),
    message("f-inbox", 60 * 200, person("Peter Schulz", "peter@schulz-law.example"), "Re: Contract draft",
      lines("Dear Alex,", "", "I went through the draft and marked two clauses. Happy to discuss on a call.", "", "Peter Schulz")),
    message("f-inbox", 60 * 260, person("Design Weekly", "hello@designweekly.example"), "Five calm interfaces",
      lines("This week: five apps that get out of your way, and why less chrome reads faster.")),
    message("f-drafts", 12, me, "Re: Lunch on Thursday?",
      `<div id="jm-body">Sounds great, Thursday at 12:30 works for me.<br><br>See you there!</div>`,
      { isDraft: true, from: null, toRecipients: [person("Maria Lopez", "maria@lopez.example")] }),
    message("f-sent", 60 * 22, me, "Quote for the website relaunch",
      lines("Hi Tom,", "", "attached is our quote for the relaunch."), { conversationId: "conv-quote" }),
    message("f-sent", 60 * 19, me, "Re: Feedback on the prototype",
      lines("Hi Sam,", "", "thanks for testing! Agreed on the settings page: I will split it into two tabs and send you a new build on Friday.", "", "Alex Morgan", "Northwind Studio", "northwind.example") +
        `<hr><div><b>From:</b> Sam Carter<br><b>Subject:</b> Feedback on the prototype</div>`,
      { conversationId: "conv-prototype", toRecipients: [person("Sam Carter", "sam@carter.example")] }),
    message("f-archive", 60 * 400, person("Northwind Bank", "service@bank.example"), "Your August statement", lines("August.")),
  ],
};

const billing = {
  folders: [folder("b-inbox", "Inbox"), folder("b-drafts", "Drafts"), folder("b-sent", "Sent Items"), folder("b-archive", "Archive"), folder("b-junk", "Junk Email"), folder("b-trash", "Deleted Items")],
  wellKnown: { inbox: "b-inbox", drafts: "b-drafts", sentitems: "b-sent", archive: "b-archive", junkemail: "b-junk", deleteditems: "b-trash" },
  messages: [
    message("b-archive", 60 * 30, person("Jonas Weber", "jonas@weber-studio.example"), "Invoice September", lines("Invoice attached."),
      { hasAttachments: true, attachments: [pdf("invoice-2026-09.pdf", 96_000)] }),
  ],
};

const boxes = { "demo-alex": alex, "demo-billing": billing };

function countFolders(box) {
  for (const f of box.folders) {
    const inside = box.messages.filter((m) => m.parentFolderId === f.id);
    f.totalItemCount = inside.length;
    f.unreadItemCount = inside.filter((m) => !m.isRead && !m.isDraft).length;
  }
}

const summary = ({ body, attachments, ...m }) => m;
// Graph's uniqueBody: what a message adds; the demo's answers put the quote below an <hr>
const unique = (m) => ({ ...summary(m), uniqueBody: { contentType: "html", content: m.body.content.split(/<hr/i)[0] } });
const byDate = (a, b) => b.receivedDateTime.localeCompare(a.receivedDateTime);
const folderId = (box, key) => box.wellKnown[key] || key;

function send(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(body === undefined ? "" : JSON.stringify(body));
}

function list(box, url, folder) {
  let items = box.messages.filter((m) => (folder ? m.parentFolderId === folderId(box, folder) : true));
  const filter = url.searchParams.get("$filter") || "";
  const conversation = filter.match(/conversationId eq '([^']*)'/);
  if (conversation) return { value: items.filter((m) => m.conversationId === conversation[1]).map(unique) };
  if (filter.includes("flag/flagStatus eq 'flagged'")) items = items.filter((m) => m.flag.flagStatus === "flagged");
  if (filter.includes("isRead eq false")) items = items.filter((m) => !m.isRead);
  if (filter.includes("hasAttachments eq true")) items = items.filter((m) => m.hasAttachments);
  const search = (url.searchParams.get("$search") || "").replace(/"/g, "").toLowerCase();
  if (search) {
    const words = search.split(/\s+/).filter((w) => w && !/[:<>]/.test(w) && w !== "and");
    items = items.filter((m) => words.every((w) => `${m.subject} ${m.bodyPreview} ${m.from?.emailAddress.name}`.toLowerCase().includes(w)));
  }
  const top = Number(url.searchParams.get("$top") || 25);
  return { value: items.sort(byDate).slice(0, top).map(summary) };
}

function reply(box, original, all) {
  const draft = message("f-drafts", 0, me, `RE: ${original.subject.replace(/^(re|aw):\s*/i, "")}`,
    `<hr><div><b>From:</b> ${original.from.emailAddress.name}<br><b>Subject:</b> ${original.subject}</div><br>${original.body.content.replace(/<\/?(html|body)>/g, "")}`,
    { isDraft: true, from: null, toRecipients: [original.from, ...(all ? original.ccRecipients : [])], conversationId: original.conversationId });
  box.messages.push(draft);
  return draft;
}

const server = http.createServer((req, res) => {
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const url = new URL(req.url, base);
    const box = boxes[(req.headers.authorization || "").replace("Bearer ", "")];
    if (!box) return send(res, 401, { error: { code: "InvalidAuthenticationToken", message: "unknown demo token" } });
    const body = raw ? JSON.parse(raw) : {};
    const path = decodeURIComponent(url.pathname.replace(/^\/v1\.0/, ""));
    const parts = path.split("/").filter(Boolean); // ["me", "messages", id, …]
    countFolders(box);

    if (path === "/me") return send(res, 200, { displayName: "Alex Morgan", mail: "alex@northwind.example" });
    if (path === "/$batch") {
      return send(res, 200, { responses: body.requests.map((r) => {
        const name = r.url.split("/")[3].split("?")[0];
        return { id: r.id, status: 200, body: { id: box.wellKnown[name] } };
      }) });
    }
    if (path === "/me/mailFolders") return send(res, 200, { value: box.folders.filter((f) => f.parentFolderId === "root") });
    if (parts[1] === "mailFolders" && parts[3] === "childFolders") {
      return send(res, 200, { value: box.folders.filter((f) => f.parentFolderId === parts[2]) });
    }
    if (parts[1] === "mailFolders" && parts[3] === "messages") return send(res, 200, list(box, url, parts[2]));
    if (path === "/me/messages" && req.method === "GET") return send(res, 200, list(box, url, null));
    if (path === "/me/messages" && req.method === "POST") {
      const attachments = (body.attachments || []).map(uploaded);
      const draft = message("f-drafts", 0, me, body.subject || "", body.body?.content || "", {
        isDraft: true, from: null, toRecipients: body.toRecipients || [], ccRecipients: body.ccRecipients || [],
        attachments, hasAttachments: attachments.some((x) => !x.isInline),
      });
      box.messages.push(draft);
      return send(res, 201, draft);
    }

    const m = box.messages.find((x) => x.id === parts[2]);
    if (parts[1] === "messages" && !m) return send(res, 404, { error: { code: "ErrorItemNotFound", message: "not found" } });
    if (parts.length === 3 && req.method === "GET") return send(res, 200, m);
    if (parts.length === 3 && req.method === "PATCH") {
      if (body.isRead !== undefined) m.isRead = body.isRead;
      if (body.flag) m.flag = body.flag;
      if (body.subject !== undefined) m.subject = body.subject;
      if (body.body) m.body = { contentType: "html", content: body.body.content };
      for (const k of ["toRecipients", "ccRecipients", "bccRecipients"]) if (body[k]) m[k] = body[k];
      m.lastModifiedDateTime = ago(0);
      return send(res, 200, m);
    }
    if (parts.length === 3 && req.method === "DELETE") {
      box.messages.splice(box.messages.indexOf(m), 1);
      return send(res, 204);
    }
    if (parts[3] === "attachments" && parts[5] === "$value") {
      const a = m.attachments.find((x) => x.id === parts[4]);
      if (!a) return send(res, 404, { error: { code: "ErrorItemNotFound", message: "not found" } });
      res.writeHead(200, { "content-type": a.contentType });
      const bytes = a.bytes || (a.contentBytes && Buffer.from(a.contentBytes, "base64"));
      return res.end(bytes || `(demo content of ${a.name})`);
    }
    if (parts[3] === "attachments" && req.method === "GET") return send(res, 200, { value: m.attachments.map(({ bytes, contentBytes, ...a }) => a) });
    if (parts[3] === "attachments" && req.method === "POST") {
      const a = uploaded(body);
      m.attachments.push(a);
      m.hasAttachments = m.attachments.some((x) => !x.isInline);
      return send(res, 201, a);
    }
    if (parts[3] === "attachments" && parts.length === 5 && req.method === "DELETE") {
      m.attachments = m.attachments.filter((x) => x.id !== parts[4]);
      m.hasAttachments = m.attachments.some((x) => !x.isInline);
      return send(res, 204);
    }
    if (parts[3] === "move") {
      m.parentFolderId = folderId(box, body.destinationId);
      return send(res, 201, m);
    }
    if (parts[3] === "createReply" || parts[3] === "createReplyAll") return send(res, 201, reply(box, m, parts[3] === "createReplyAll"));
    if (parts[3] === "createForward") {
      const draft = reply(box, m, false);
      draft.subject = `FW: ${m.subject}`;
      draft.toRecipients = body.toRecipients || [];
      return send(res, 201, draft);
    }
    if (parts[3] === "send") {
      m.isDraft = false;
      m.parentFolderId = "f-sent";
      return send(res, 202);
    }
    send(res, 404, { error: { code: "NotImplemented", message: `${req.method} ${path} is not part of the demo` } });
  });
});

server.listen(port, "127.0.0.1", () => console.log(`demo Graph on ${base}`));
