"use strict";
// The page's session with the server: taking one (with a key, an invite,
// the Master's pairing code or the Host's own), renewing a refused one,
// ending one nobody holds, and what the door says while signed out. Loaded
// right after state.js (index.html), whose `state` it reads and writes; split
// from state.js without changing a line of it.

function adminHeaders(base) {
  const h = base || {};
  // Once a session exists it is the only credential sent. In particular, the
  // root/bootstrap credential must not keep riding along with every browser request.
  if (state.session) {
    h["x-session-token"] = state.session;
  } else if (state.roomToken) {
    h["x-room-token"] = state.roomToken;
  }
  return h;
}

// Exchange the room key for a session (server-derived identity). Admin
// sessions persist across deploys until their selected expiry; invite sessions
// can be renewed from their local invite after a restart.
//
// An attempt belongs to the person, key and server it was started for. Its
// answer may arrive after that person signed out and someone else signed in;
// applied then, it would put the last person's session and name over the new
// one's. So every answer is checked against the generation and server it was
// asked under, and one that is no longer theirs writes nothing: not the
// identity, not the stored session, not a verdict.
//
// Resolves true when the outcome is this page's current identity state (read
// it from state.authStatus as before), false when the attempt was overtaken
// and the caller must stop: whatever it was doing was for someone who is gone.
let sessionTaking = null;   // { key, promise } while an attempt is out
function takeSession() {
  // Room navigation is not a new login. Keep the already-bound identity;
  // clearing it here made every loca click throw the master back to the door.
  if (!state.roomToken && !state.pairing && state.session) {
    state.authStatus = "authenticated";
    return Promise.resolve(true);
  }
  const gen = state.buildingGen;
  const server = serverBase();
  // The same person asking again while their attempt is out joins it. Two
  // exchanges of one one-use pairing code would end with the second one's
  // refusal written over the first one's session.
  const key = JSON.stringify([gen, server, state.name, state.roomToken, state.pairing]);
  if (sessionTaking && sessionTaking.key === key) return sessionTaking.promise;
  const current = () => !buildingStale(gen) && server === serverBase();
  const taking = { key, promise: null };
  taking.promise = takeSessionFor(server, current).finally(() => {
    if (sessionTaking === taking) sessionTaking = null;
  });
  sessionTaking = taking;
  return taking.promise;
}
async function takeSessionFor(server, current, hostRenewed = false) {
  state.authStatus = "unknown";
  state.session = null;
  state.adminSession = false;
  state.sessionExpires = null;
  if (!state.roomToken && !state.pairing) {
    // Whether this page still held a stored Master session when the attempt
    // began. Loca Host replaces one that has ended (hostMasterSession); it
    // gives none to a page that holds none: a person who signed out stays
    // signed out, through a reload too.
    let heldStored = false;
    // Keep the expiring stand-in for exactly the lifetime selected at pairing.
    // This is NOT the root key: it is revocable server-side and carries its own
    // absolute expiry. localStorage lets that chosen lifetime survive closing
    // the tab/browser; logout and 401 remove it immediately.
    try {
      const cached = JSON.parse(
        localStorage.getItem("loca-admin-session")
        || sessionStorage.getItem("loca-admin-session")
        || "null"
      );
      heldStored = !!cached?.token;
      if (cached?.token && Number(cached.expiresAt) > Date.now()) {
        state.session = cached.token;
        state.adminSession = true;
        state.sessionExpires = Number(cached.expiresAt);
        // A cached token is provisional until the server resolves it. Network
        // and 5xx failures keep auth unknown; only an explicit 401 means absent.
        let who;
        const whoController = new AbortController();
        const whoTimeout = setTimeout(() => whoController.abort(), 2000);
        try {
          who = await fetch(server + "/whoami", {
            headers: { "x-session-token": state.session },
            signal: whoController.signal,
          });
        } catch (e) {
          return current();
        } finally {
          clearTimeout(whoTimeout);
        }
        if (!current()) return false;
        if (who.status === 401) {
          state.session = null;
          state.adminSession = false;
          state.sessionExpires = null;
          state.authStatus = "unauthenticated";
          localStorage.removeItem("loca-admin-session");
          sessionStorage.removeItem("loca-admin-session");
          if (!hostRenewed && await hostMasterSession(server, current)) {
            return current() && takeSessionFor(server, current, true);
          }
          return current();
        }
        if (!who.ok) return true;
        const identity = await who.json();
        if (!current()) return false;
        if (identity.name) {
          state.name = identity.name;
          $("name").value = state.name;
          cached.name = state.name;
        }
        localStorage.setItem("loca-admin-session", JSON.stringify(cached));
        sessionStorage.removeItem("loca-admin-session");
        state.authStatus = "authenticated";
        return true;
      }
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    } catch (e) {
      // A body that failed after its person left is not a reason to drop the
      // session the next person stored.
      if (!current()) return false;
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    }
    if (heldStored && !hostRenewed && await hostMasterSession(server, current)) {
      return current() && takeSessionFor(server, current, true);
    }
    if (!current()) return false;
    state.authStatus = "unauthenticated";
    return true;      // open house or no current credential
  }
  // Admin wins when both fields were filled. Mixing credentials made the room
  // token win server-side while leaving the root/bootstrap credential resident in the
  // browser; one request must have one unambiguous identity.
  const adminExchange = !!state.pairing;
  try {
    // Not cancelled when the generation moves (see the fetch wrapper above):
    // the server makes the session whether or not anyone is still here, and
    // only its token can end it. Every line below checks current() after each
    // wait, and a stale answer is applied to nothing and ended.
    const r = await pageFetch(server + "/sessions", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(adminExchange ? { "x-pairing-code": state.pairing } : {}),
        ...(!adminExchange && state.roomToken ? { "x-room-token": state.roomToken } : {}),
      },
      body: JSON.stringify({ name: adminExchange ? "" : state.name, kind: "user" }),
    });
    if (!current()) {
      if (r.ok) endUnheldSession(server, (await r.json().catch(() => null))?.session_token);
      return false;
    }
    if (r.ok) {
      const info = await r.json();
      if (!current()) { endUnheldSession(server, info?.session_token); return false; }
      state.session = info.session_token || null;
      state.authStatus = state.session ? "authenticated" : "unauthenticated";
      state.adminSession = info.admin === true;
      state.sessionExpires = info.expires_at || null;
      // Credentials name the seat. Never keep a user-entered alias after the
      // server has returned the canonical session identity.
      if (info.name) {
        state.name = info.name;
        $("name").value = state.name;
      }
      // A successful pairing returns a server-confirmed admin
      // authority, so drop the raw key from memory and clear the input. From
      // here on admin rides on the session (header + WS), never the key.
      // PRINCIPLES: the key does not linger in the browser. If the session
      // expires the door asks for the key again — nothing is stored.
      if (adminExchange && state.session && state.adminSession) {
        try {
          localStorage.setItem("loca-admin-session", JSON.stringify({
            token: state.session, expiresAt: state.sessionExpires, name: state.name,
          }));
        } catch (e) {}
        state.pairing = "";
        state.roomToken = "";
        try { $("pairingCode").value = ""; } catch (e) {}
        try { $("roomToken").value = ""; } catch (e) {}
      }
    } else if (r.status === 401 || r.status === 403) {
      state.authStatus = "unauthenticated";
    }
  } catch (e) {
    // Retain unknown: transport failure is not an auth verdict. Overtaken, it
    // is not this page's failure at all.
    if (!current()) return false;
  }
  return true;
}
// Loca Host only (the desktop app with its own server): its page's Master
// session is minted by the shell from the Host's admin token, at start and,
// through this, again when it has ended (expired, or ended on the server).
// The page receives the bounded session only, never the token (main.rs
// host_master_session). Anywhere else (a browser, the desktop client) there
// is no Host to ask and this does nothing. Resolves true when a fresh session
// is stored for takeSession to use.
//
// It is asked only to replace a session this page held (takeSessionFor), and
// its answer is for whoever asked: if that person has signed out, someone else
// has signed in or the server changed while the Host was minting, the answer
// is not stored, and the session it carries is ended at the server, since
// nobody holds it.
function isLocaHostPage() {
  const status = window.__LOCA_HOST_PROVISIONING__;
  const tauri = window.__TAURI__;
  return !!status && status.state !== "not_applicable"
    && !!(tauri && tauri.core && typeof tauri.core.invoke === "function");
}
async function hostMasterSession(server, current) {
  if (!isLocaHostPage()) return false;
  try {
    const value = await window.__TAURI__.core.invoke("host_master_session");
    const minted = JSON.parse(value);
    if (!minted?.token || !(Number(minted.expiresAt) > Date.now())) return false;
    if (!current()) { endUnheldSession(server, minted.token); return false; }
    localStorage.setItem("loca-admin-session", value);
    return true;
  } catch (e) { return false; }
}
// The Host page's Master session was refused while in use: take the next one
// from the Host and carry on, the loca and the socket as they were. One at a
// time; if the Host cannot give one, the door closes as anywhere else.
let hostRenewal = null;
function renewHostSession() {
  if (hostRenewal) return hostRenewal;
  const gen = state.buildingGen;
  hostRenewal = (async () => {
    state.session = null;   // so takeSession asks again rather than keep the refused one
    if (!(await takeSession()) || buildingStale(gen)) return;
    if (state.authStatus === "authenticated" && state.session) {
      if (!steppedDown()) openWs();
      refreshRooms();
      return;
    }
    if (state.authStatus === "unauthenticated") lockRefusedSession();
  })().finally(() => { hostRenewal = null; });
  return hostRenewal;
}
// A session nobody at this tab holds any more stays valid on its server until
// it expires: an exchange answered after its person left, someone else took
// the tab or the server address changed; the session of a person who left;
// the session a new exchange replaces. End it at the server that granted it,
// with its own token and nothing else: never the current person's headers,
// never the current server. It is sent with the page's own fetch, so the next
// generation's cancel does not reach it, and kept alive past an unload. Its
// outcome is nobody's to see.
//
// This does not end every such session. A closed or crashed tab sends
// nothing; an answer that cannot be read (a broken connection, an unreadable
// body) never shows the token; a DELETE that fails is not retried. Those
// sessions stay on the server until it ends them: at their expiry, or, for an
// invite session, which has none, at a restart or when its key is revoked.
function endUnheldSession(server, token) {
  if (!token) return;
  pageFetch(server + "/sessions", {
    method: "DELETE", headers: { "x-session-token": token }, keepalive: true,
  }).catch(() => {});
}
// The Master session this browser keeps for its next load and its other tabs
// is still held, by the browser, even when this tab lets go of it.
function storedAdminSession(token) {
  try {
    const cached = JSON.parse(localStorage.getItem("loca-admin-session") || "null");
    return !!token && cached?.token === token;
  } catch (e) { return false; }
}
// Admin with a live master session, or when the server has no ADMIN_TOKEN
// configured (dev mode — open to everyone).
function isAdmin() { return state.adminSession === true || state.adminOpen === true; }
// The page's session is refused and no new one can be taken: back behind the
// door, nothing of the room shown, and the socket closed rather than retried
// with a session that will not come back.
function lockRefusedSession() {
  // A page that held no session (a first visit) has nothing that ended.
  const held = !!state.session || state.adminSession;
  if (state.adminSession) {
    try {
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    } catch (e) {}
    state.session = null;
    state.adminSession = false;
    state.sessionExpires = null;
    state.authStatus = "unauthenticated";
  }
  if (state.ws) { state.ws.onclose = null; state.ws.close(); state.ws = null; }
  setLocked(true);
  clearBuildingData();
  clearPersonWords();
  setStatus("locked — key required", false);
  // A reason the desktop shell gave for starting this page signed out (its
  // sign-in could not be kept across a reload: main.rs KEYCHAIN_SHIM) stays
  // on the sign-in view until the person signs in again.
  if (window.__LOCA_SIGNIN_NOTICE__) {
    $("doorline").textContent = String(window.__LOCA_SIGNIN_NOTICE__);
    return;
  }
  $("doorline").innerHTML = signedOutDoorline(held);
}
// The door's line for someone not signed in. With a key, the key itself was
// just refused; without one (a Master pairing, which is used once) the
// session ended and a new code is needed. Loca Host signs its Master in
// itself when it starts (main.rs); its person holds no pairing code, so the
// Host's door names the way that works.
function signedOutDoorline(held) {
  const host = isLocaHostPage();
  return state.roomToken
    ? 'that key does not open this table.'
    : held ? (host ? `your session ended.<br>${HOST_SIGN_IN}` : 'your session ended.<br>enter a fresh master pairing code or loca key.')
    : host ? `you are signed out.<br>${HOST_SIGN_IN}`
    : 'a private table for people and agents.<br>sign in with your key, a loca invite or a Master pairing code.';
}
// The line index.html starts with, before the page knows who is here.
const INITIAL_DOORLINE = document.getElementById("doorline")?.innerHTML || "";
const HOST_SIGN_IN = "close and reopen Loca Host to sign in again as Master, or sign in with a key.";

// The page's current session is no longer accepted (it expired, or was
// ended), and the page holds a re-usable key (an invite, a membership key, the
// sandbox room key): take a new session with it and carry on, the loca and
// whatever was typed left as they are, the socket reopened. Every trigger
// (the poll's 401, a socket the server closed) joins the one renewal out.
// Only the page's current session is renewed, never one replaced since, and
// a key the server refuses locks the door once: there is no second try.
// Resolves "renewed", "refused" or "" (nothing to do, overtaken, or no
// verdict from the network: the health poll asks again, retryOwedRenewal).
//
// A renewal belongs to the person (the generation), the server and the key it
// was started for. One out for an earlier person is never joined by, never
// blocks and never applies to whoever is here now: they get their own.
function renewalKey() { return JSON.stringify([state.buildingGen, serverBase(), state.roomToken]); }
let sessionRenewal = null;   // { key, promise } while a renewal is out
let owedRenewal = "";        // the key of a renewal the network left unanswered
function renewRefusedSession(refused) {
  const key = renewalKey();
  if (sessionRenewal && sessionRenewal.key === key) return sessionRenewal.promise;
  if (!state.roomToken || state.pairing || state.session !== refused) return Promise.resolve("");
  // No session at all: only when this key's last renewal got no answer.
  if (!refused && owedRenewal !== key) return Promise.resolve("");
  const gen = state.buildingGen;
  const renewal = { key, promise: null };
  renewal.promise = (async () => {
    // The refused session is being replaced: end it (the server has said it
    // is gone; this only makes sure).
    endUnheldSession(state.server || serverBase(), refused);
    if (!(await takeSession()) || buildingStale(gen)) return "";
    if (state.authStatus === "authenticated" && state.session) {
      owedRenewal = "";
      // A tab that stepped down for a newer connection (onEvicted) stays
      // out: the new session does not take the seat back by itself.
      if (!steppedDown()) openWs();
      return "renewed";
    }
    if (state.authStatus === "unauthenticated") {
      owedRenewal = "";
      lockRefusedSession();
      return "refused";
    }
    // No verdict (the network): still this person's, still reconnecting.
    owedRenewal = key;
    setStatus("reconnecting…", false);
    return "";
  })().finally(() => { if (sessionRenewal === renewal) sessionRenewal = null; });
  sessionRenewal = renewal;
  return renewal.promise;
}
// A renewal the network left without a verdict is tried again by the health
// poll on its own: once per tick at most, never two at once, and only for the
// same person and key. (The poll's /rooms read may well succeed with the raw
// key meanwhile, so a 401 cannot be waited for.)
function retryOwedRenewal() {
  if (owedRenewal && owedRenewal === renewalKey() && !state.session && !sessionTaking) {
    renewRefusedSession(null);
  }
}
