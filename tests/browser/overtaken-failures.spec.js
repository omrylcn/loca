const { test, expect } = require("@playwright/test");
const { MASTER, smasterKey, freshDoor, signInWithKey, expectSignedInAs, signOut, switchTo, settle, expectStill } = require("./support/switch");

// A request cancelled because the tab moved on to someone else has failed for
// nobody. Each test starts one request for the first person (or for whoever
// stood at the door), holds it, lets someone else take the tab, which cancels
// it, and checks that its failure branch told the person now at the tab
// nothing: no door or join-request line, no connection status, no system line
// in the conversation, no pane or dialog error, no alert. Everything written
// into the page from the switch on is watched, so a warning that a later
// render would paint over is still caught.

async function signInAsMaster(page, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  await page.evaluate(code => {
    setEntryPath("master");
    document.getElementById("pairingCode").value = code;
  }, pairing_code);
  await page.locator("#connectBtn").click();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
}

// Record every text or aria-label written into the page that contains `banned`.
async function watchFor(page, banned) {
  await page.evaluate(word => {
    const seen = window.__banned = [];
    const hit = text => typeof text === "string" && text.toLowerCase().includes(word.toLowerCase());
    new MutationObserver(records => {
      for (const r of records) {
        if (r.type === "attributes") {
          const label = r.target.getAttribute(r.attributeName);
          if (hit(label)) seen.push(`${r.target.id || r.target.className}[${r.attributeName}]: ${label}`);
          continue;
        }
        const el = r.target.nodeType === 1 ? r.target : r.target.parentElement;
        const text = r.type === "characterData" ? r.target.data : el && el.textContent;
        if (hit(text)) {
          for (const n of r.addedNodes) if (hit(n.textContent)) seen.push(`${el.id || el.className}: ${n.textContent}`);
          if (r.type === "characterData") seen.push(`${el && (el.id || el.className)}: ${text}`);
        }
      }
    }).observe(document.documentElement, {
      subtree: true, childList: true, characterData: true,
      attributes: true, attributeFilter: ["aria-label", "data-tip"],
    });
  }, banned);
}

// Hold the first matching request until the test lets it go; it never
// reaches the server.
async function hold(page, glob, method) {
  const h = {};
  h.arrived = new Promise(resolve => { h.seen = resolve; });
  const released = new Promise(resolve => { h.release = resolve; });
  let taken = false;
  await page.route(glob, async route => {
    if (route.request().method() !== method || taken) return route.fallback();
    taken = true;
    h.seen();
    await released;
    await route.abort("connectionreset").catch(() => {});
  });
  return h;
}

const ABORTED = "aborted";   // the message any cancelled fetch carries

const CASES = [
  {
    name: "a join request sent from the door",
    who: "door", end: "typed-over", glob: "**/join-requests", method: "POST", banned: "could not reach the server",
    start: async (page, tag) => {
      await page.locator("#name").fill(`${tag}-asker`);
      await page.locator("#joinRequestBtn").click();
    },
  },
  {
    name: "an approved join request collecting its key",
    who: "door", end: "typed-over", glob: "**/join-requests/*/bootstrap", method: "POST", banned: "could not reach the server",
    start: async (page, tag, request) => {
      await page.locator("#name").fill(`${tag}-asker`);
      await page.locator("#joinRequestBtn").click();
      await expect(page.locator("#joinRequestStatus")).toContainText("waiting for a Master");
      expect((await request.post("/admission-stock", { headers: MASTER, data: { count: 1 } })).ok()).toBeTruthy();
      const pending = await (await request.get("/join-requests", { headers: MASTER })).json();
      const asked = pending.pending.find(r => r.name === `${tag}-asker`);
      expect((await request.post(`/join-requests/${asked.id}/approve`, { headers: MASTER })).ok()).toBeTruthy();
    },
  },
  {
    name: "a refresh of the loca list",
    who: "smaster", end: "sign-out", glob: "**/rooms", method: "GET", banned: "cannot reach server",
    start: page => page.evaluate(() => { refreshRooms(); }),
  },
  {
    name: "suspending a member",
    who: "master", end: "sign-out", glob: "**/members/*/suspension", method: "POST", banned: ABORTED, confirm: true,
    start: page => page.evaluate(() => {
      const b = document.createElement("button");
      Object.assign(b.dataset, { suspendMember: "pr_synthetic", memberName: "synthetic", suspended: "1" });
      document.getElementById("peopleList").appendChild(b);
      b.click();
    }),
  },
  {
    name: "revoking a membership",
    who: "master", end: "sign-out", glob: "**/members/*", method: "DELETE", banned: ABORTED, confirm: true,
    start: page => page.evaluate(() => {
      const b = document.createElement("button");
      Object.assign(b.dataset, { revokeMember: "pr_synthetic", memberName: "synthetic" });
      document.getElementById("peopleList").appendChild(b);
      b.click();
    }),
  },
  {
    name: "changing the lead from the picker",
    who: "smaster", end: "sign-out", glob: "**/rooms/*/lead", method: "POST", banned: "could not change lead",
    start: page => page.evaluate(() => {
      const select = document.getElementById("leadSelect");
      select.value = "";
      select.dispatchEvent(new Event("change"));
    }),
  },
  {
    name: "a reaction",
    who: "smaster", end: "sign-out", glob: "**/rooms/*/messages/*/reactions", method: "POST", banned: "reaction failed",
    start: page => page.evaluate(() => {
      msgById.set(987654, { id: 987654, sender: "synthetic-peer" });
      const b = document.createElement("button");
      Object.assign(b.dataset, { react: "987654", emoji: "👍" });
      document.getElementById("feed").appendChild(b);
      b.click();
    }),
  },
  {
    name: "calling someone into the loca",
    who: "smaster", end: "sign-out", glob: "**/rooms/*/call", method: "POST", banned: "call failed",
    start: page => page.evaluate(() => { callIn("synthetic-callee"); }),
  },
  {
    name: "downloading an attachment",
    who: "smaster", end: "sign-out", glob: "**/attachments/att_synthetic_dl", method: "GET", banned: "could not download",
    start: page => page.evaluate(() => {
      const chip = attachmentChip(state.room, { id: "att_synthetic_dl", name: "synthetic.txt", size: 3 });
      document.getElementById("feed").appendChild(chip);
      chip.click();
    }),
  },
  {
    name: "opening an attachment",
    who: "smaster", end: "sign-out", glob: "**/attachments/att_synthetic_open", method: "GET", banned: "could not open",
    start: page => page.evaluate(() => { openAttachment(state.room, { id: "att_synthetic_open", name: "synthetic.txt" }); }),
  },
  {
    name: "a note's history",
    who: "smaster", end: "sign-out", glob: "**/notes/synthetic-note/history", method: "GET", banned: "history failed",
    start: page => page.evaluate(() => {
      const box = document.createElement("div");
      box.id = "nhist-synthetic-note";
      box.className = "hidden";
      document.getElementById("noteList").appendChild(box);
      toggleHistory("synthetic-note");
    }),
  },
  {
    name: "creating a loca",
    who: "smaster", end: "sign-out", glob: "**/rooms", method: "POST", banned: "could not be confirmed",
    start: (page, tag) => page.evaluate(name => {
      document.getElementById("extCreateName").value = name;
      submitCreateLoca();
    }, `${tag}-loca`),
    // Not left disabled for whoever uses the dialog next.
    after: async page => expect(await page.evaluate(() => document.getElementById("extCreateSubmit").disabled)).toBe(false),
  },
  {
    name: "adding an extension",
    who: "smaster", end: "sign-out", glob: "**/rooms/*/extensions/notes", method: "PUT", banned: "could not reach the server",
    start: page => page.evaluate(() => { changeExtension("notes", true); }),
  },
  {
    name: "saving reminder settings",
    who: "smaster", end: "sign-out", glob: "**/rooms/*/settings", method: "PUT", banned: "could not reach the loca server",
    start: page => page.evaluate(() => { applyReminderSettings(); }),
    // No "Saving…" left behind either.
    after: async page => expect(await page.evaluate(() => document.getElementById("reminderSaveState").textContent)).toBe(""),
  },
  {
    name: "saving one's appearance",
    who: "smaster", end: "sign-out", glob: "**/profile/preferences", method: "PUT", banned: "not saved to your profile",
    start: page => page.evaluate(() => { saveAppearance("density", "compact"); }),
  },
  {
    name: "creating a note, typed over by another key",
    who: "smaster", end: "typed-over", glob: "**/rooms/*/notes", method: "POST", banned: ABORTED,
    start: page => page.evaluate(() => {
      document.getElementById("nnKey").value = "synthetic-note";
      createNote();
    }),
  },
  {
    name: "reading loca memory, typed over by another key",
    who: "smaster", end: "typed-over", glob: "**/rooms/*/memory", method: "GET", banned: ABORTED,
    start: page => page.evaluate(() => { fetchMemory(); }),
  },
];

for (const [index, c] of CASES.entries()) {
test(`${c.name}, cancelled by someone else taking the tab, reports nothing to them`, async ({ browser, request }) => {
  test.setTimeout(60_000);
  const tag = `ovf${index}`;
  const firstKey = await smasterKey(request, `${tag}-first`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const next = `${tag}-next`;
  const { context, page, dialogs, answer } = await freshDoor(browser);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));

  if (c.who === "smaster") {
    await signInWithKey(page, firstKey);
    await expectSignedInAs(page, `${tag}-first`);
  } else if (c.who === "master") {
    await signInAsMaster(page, request);
  }
  await settle(page, 300);

  const held = await hold(page, c.glob, c.method);
  if (c.confirm) answer.yes = true;
  await c.start(page, tag, request);
  await held.arrived;
  const dialogsBefore = dialogs.length;
  await watchFor(page, c.banned);

  // Someone else takes the tab: the request is cancelled now.
  let mine;
  if (c.end === "sign-out" && c.who === "master") {
    // The Master leaves; the next person goes back to the Smaster door.
    await signOut(page);
    await page.locator("#masterEntryButton").click();
    await signInWithKey(page, nextKey);
    await expectSignedInAs(page, next);
    mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  } else if (c.end === "sign-out") {
    mine = await switchTo(page, nextKey, next);
  } else {
    await signInWithKey(page, nextKey);
    await expectSignedInAs(page, next);
    mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  }
  held.release();
  await settle(page);

  expect(await page.evaluate(() => window.__banned)).toEqual([]);
  expect((await page.content()).toLowerCase()).not.toContain(c.banned.toLowerCase());
  expect(dialogs.slice(dialogsBefore)).toEqual([]);
  expect(errors).toEqual([]);
  if (c.after) await c.after(page);
  await expectStill(page, mine);
  await context.close();
});
}
