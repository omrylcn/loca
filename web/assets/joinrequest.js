"use strict";
// Human browser join request. Agent onboarding uses the identity-scoped skill.
//
// An outside agent names itself and asks to join the Building; the request
// grants nothing until a Master approves it, which consumes one admission-stock
// right and issues a Lobby membership (mb_). The agent then collects that mb_
// exactly once via bootstrap and takes its seat.
//
// This is ADDITIVE: it never replaces the door's existing pairing-code / invite
// flow. On success it simply feeds the door — fills the name + the membership
// token and clicks "take your seat" — so the existing takeSession() path does
// the actual connect. The per-request secret lives only in this tab (memory +
// sessionStorage); it is not a Building credential.
//
// The server keeps a copy of the key until the requester acknowledges it, or
// for 24 hours after the approval. The ACK is sent only once the key is known
// to be kept: the door's sign-in succeeded with that very key as the name
// that asked (the server names the session it grants), and the seat the page
// keeps across reloads (localStorage "loca-seat") reads back with this name
// and this key. Until the ACK is answered the request's id and secret stay in
// this tab, so a failed ACK, or a reload before it was answered, is retried
// the next time that same person's seat is taken here. Someone else signing
// in at the tab sends nothing, and neither does the same name signing in
// with a different key before the collected one was seen kept.
(function () {
  function $(id) {
    return document.getElementById(id);
  }
  function base() {
    if (typeof serverBase === "function") return serverBase();
    var el = $("server");
    return (el && el.value.trim()) || "";
  }
  // `phase` ("waiting" while a Master has the request, "approved" while the
  // key is collected) draws the waiting card around the line
  // (#humanJoinRequest[data-phase], page-entry.css); any other line ends it.
  function setStatus(msg, phase) {
    var el = $("joinRequestStatus");
    if (el) el.textContent = msg || "";
    var box = $("humanJoinRequest");
    if (box) box.setAttribute("data-phase", phase || "");
  }

  var EXPIRED =
    "you were approved, but the key was not collected within 24 hours, so this server " +
    "no longer holds it — ask a Master to give you access again";

  // Requests whose key was collected and whose ACK is still owed: id ->
  // { secret, name, server, kept }. `kept` turns true once this page has seen
  // the collected key itself signed in with and held in the kept seat; the
  // key stays in memory only until then, for that comparison, and is never
  // written here. The record is in sessionStorage so a reload can finish an
  // ACK that failed; in memory too, so a browser that refuses storage still
  // finishes it in this page.
  var ACK_PREFIX = "loca-join-ack-";
  var owedAcks = {};
  var collectedKeys = {};
  function storeOwed(id) {
    try {
      sessionStorage.setItem(ACK_PREFIX + id, JSON.stringify(owedAcks[id]));
    } catch (e) {}
  }
  function oweAck(id, record, key) {
    owedAcks[id] = record;
    collectedKeys[id] = key;
    storeOwed(id);
  }
  function ackPaid(id) {
    delete owedAcks[id];
    delete collectedKeys[id];
    try {
      sessionStorage.removeItem(ACK_PREFIX + id);
    } catch (e) {}
  }
  function acksOwed() {
    var all = {};
    try {
      for (var i = 0; i < sessionStorage.length; i++) {
        var key = sessionStorage.key(i);
        if (key && key.indexOf(ACK_PREFIX) === 0) {
          var record = JSON.parse(sessionStorage.getItem(key) || "null");
          if (record && record.secret && record.name) all[key.slice(ACK_PREFIX.length)] = record;
        }
      }
    } catch (e) {}
    for (var id in owedAcks) all[id] = owedAcks[id];
    return all;
  }

  // The seat the page restores after a reload holds this name and this key.
  function seatHolds(name, key) {
    var seat = null;
    try {
      seat = JSON.parse(localStorage.getItem("loca-seat") || "null");
    } catch (e) {
      return false;
    }
    return !!seat && seat.name === name && seat.roomToken === key;
  }

  async function sendOwedAcks(gen) {
    var owed = acksOwed();
    for (var id in owed) {
      if (buildingStale(gen)) return;
      var record = owed[id];
      // Only for the person who asked, signed in on the server that was asked.
      if (state.authStatus !== "authenticated") continue;
      if (record.name !== state.name || record.server !== state.server) continue;
      if (!record.kept) {
        var collected = collectedKeys[id];
        // Nothing to compare with: a reload came before the key was seen
        // kept. This page cannot tell that the key survived, so it does not
        // say so; the server ends its copy at the deadline instead.
        if (!collected) continue;
        // Signed in as the same name with some other key: that says nothing
        // about the collected one.
        if (state.roomToken !== collected) continue;
        if (!seatHolds(record.name, collected)) {
          setStatus("you are in, but this browser could not keep your key — a reload would lose it");
          continue;
        }
        record.kept = true;
        owedAcks[id] = record;
        delete collectedKeys[id];
        storeOwed(id);
      }
      try {
        var r = await fetch(
          record.server + "/join-requests/" + encodeURIComponent(id) + "/ack",
          { method: "POST", headers: { "x-join-secret": record.secret } }
        );
        // 404: the server knows no such request for this secret; there is
        // nothing left to acknowledge. Anything else unanswered stays owed.
        if (r.ok || r.status === 404) ackPaid(id);
      } catch (e) {
        /* stays owed: retried when this person's seat is next taken here */
      }
    }
  }
  document.addEventListener("loca:seat-taken", function (event) {
    sendOwedAcks(event.detail ? event.detail.gen : state.buildingGen);
  });

  var poll = null;
  function stopPoll() {
    if (poll) {
      clearInterval(poll);
      poll = null;
    }
  }

  async function startJoinRequest() {
    var name = (($("name") && $("name").value) || "").trim();
    if (!name) {
      setStatus("enter your name first");
      return;
    }
    var server = base();
    if (!server) {
      setStatus("enter the server address first");
      return;
    }
    var who = $("joinRequestName");
    if (who) who.textContent = name;
    setStatus("sending your request…", "waiting");
    var gen = state.buildingGen;
    try {
      var r = await fetch(server + "/join-requests", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name: name, kind: "user" }),
      });
      if (r.status === 429) {
        setStatus("the building is busy — try again shortly");
        return;
      }
      if (r.status === 409) {
        setStatus("this identity already exists — use its existing login credential; do not create another name to regain access");
        return;
      }
      if (!r.ok) {
        setStatus("could not send the request: " + (await r.text()));
        return;
      }
      var data = await r.json();
      try {
        sessionStorage.setItem("loca-join-" + data.request_id, data.request_secret);
      } catch (e) {}
      setStatus("waiting for a Master to approve you…", "waiting");
      pollUntilDecided(server, data.request_id, data.request_secret, name);
    } catch (e) {
      // Cancelled because someone else took the tab: not their failure.
      if (!buildingStale(gen)) setStatus("could not reach the server");
    }
  }

  // A request waits for whoever filed it at this door. If someone signs in at
  // this tab meanwhile, the page is theirs: an approval that arrives then must
  // not collect the key and take the seat over them. The watch stops; the
  // request itself stays, and asking again from the door resumes it.
  function pollUntilDecided(server, id, secret, name) {
    stopPoll();
    var gen = state.buildingGen;
    poll = setInterval(async function () {
      if (buildingStale(gen)) { stopPoll(); return; }
      try {
        var r = await fetch(
          server + "/join-requests/" + encodeURIComponent(id),
          { headers: { "x-join-secret": secret } }
        );
        if (r.status === 404) {
          setStatus("this request is no longer valid");
          stopPoll();
          return;
        }
        if (buildingStale(gen)) { stopPoll(); return; }
        if (!r.ok) return; // keep polling through transient errors
        var s = await r.json();
        if (buildingStale(gen)) { stopPoll(); return; }
        if (s.status === "denied") {
          setStatus("a Master declined the request");
          stopPoll();
          return;
        }
        if (s.status === "approved" && s.bootstrap_ready) {
          stopPoll();
          await bootstrapAndSeat(server, id, secret, name, gen);
          return;
        }
        if (s.status === "approved" && s.bootstrap_ready === false) {
          // Nothing more will come: the key was handed over and acknowledged
          // already, or its delivery window closed.
          stopPoll();
          setStatus(
            s.bootstrap_expired
              ? EXPIRED
              : "this request was already completed — sign in with the key it gave you, or ask a Master to give you access again"
          );
        }
      } catch (e) {
        /* transient — keep polling */
      }
    }, 3000);
  }

  async function bootstrapAndSeat(server, id, secret, name, gen) {
    if (buildingStale(gen)) return;
    setStatus("approved — collecting your key…", "approved");
    try {
      var r = await fetch(
        server + "/join-requests/" + encodeURIComponent(id) + "/bootstrap",
        { method: "POST", headers: { "x-join-secret": secret } }
      );
      if (buildingStale(gen)) return;
      if (r.status === 410) {
        setStatus(EXPIRED);
        return;
      }
      if (r.status === 503) {
        // The server could not read the request just now; the key is still
        // there for this request. Keep watching and collect it then.
        setStatus("the server could not hand over your key just now — trying again…", "approved");
        pollUntilDecided(server, id, secret, name);
        return;
      }
      if (!r.ok) {
        var why = await r.text();
        if (!buildingStale(gen)) setStatus("could not collect the key: " + why);
        return;
      }
      var data = await r.json();
      if (buildingStale(gen)) return;
      // Owed from now on; sent when the seat below is taken and kept.
      oweAck(id, { secret: secret, name: name, server: server, kept: false }, data.davet);
      try {
        sessionStorage.removeItem("loca-join-" + id);
      } catch (e) {}
      // Feed the existing door: the mb_ is a building (Lobby) membership token.
      if ($("name")) $("name").value = name;
      if ($("roomToken")) $("roomToken").value = data.davet;
      if ($("pairingCode")) $("pairingCode").value = "";
      setStatus("joined — taking your seat in the Lobby…", "approved");
      if ($("connectBtn")) $("connectBtn").click();
    } catch (e) {
      if (!buildingStale(gen)) setStatus("could not reach the server");
    }
  }

  window.addEventListener("load", function () {
    var btn = $("joinRequestBtn");
    if (btn) btn.addEventListener("click", startJoinRequest);
  });
})();
