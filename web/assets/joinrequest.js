"use strict";
// Agent-initiated join request (the loca "join-request" model).
//
// An outside agent names itself and asks to join the Building; the request
// grants nothing until a Master approves it, which consumes one admission-stock
// right and issues a Lobby membership (mb_). The agent then collects that mb_
// exactly once via bootstrap and takes its seat.
//
// This is ADDITIVE: it never replaces the door's existing pairing-code / davet
// flow. On success it simply feeds the door — fills the name + the membership
// token and clicks "take your seat" — so the existing takeSession() path does
// the actual connect. The per-request secret lives only in this tab (memory +
// sessionStorage) and is discarded once bootstrapped; it is not a Building
// credential.
(function () {
  function $(id) {
    return document.getElementById(id);
  }
  function base() {
    if (typeof serverBase === "function") return serverBase();
    var el = $("server");
    return (el && el.value.trim()) || "";
  }
  function setStatus(msg) {
    var el = $("joinRequestStatus");
    if (el) el.textContent = msg || "";
  }

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
    setStatus("sending your request…");
    try {
      var r = await fetch(server + "/join-requests", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name: name, kind: "agent" }),
      });
      if (r.status === 429) {
        setStatus("the building is busy — try again shortly");
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
      setStatus("waiting for a Master to approve you…");
      pollUntilDecided(server, data.request_id, data.request_secret, name);
    } catch (e) {
      setStatus("could not reach the server");
    }
  }

  function pollUntilDecided(server, id, secret, name) {
    stopPoll();
    poll = setInterval(async function () {
      try {
        var r = await fetch(
          server +
            "/join-requests/" +
            encodeURIComponent(id) +
            "?secret=" +
            encodeURIComponent(secret)
        );
        if (r.status === 404) {
          setStatus("this request is no longer valid");
          stopPoll();
          return;
        }
        if (!r.ok) return; // keep polling through transient errors
        var s = await r.json();
        if (s.status === "denied") {
          setStatus("a Master declined the request");
          stopPoll();
          return;
        }
        if (s.status === "approved" && s.bootstrap_ready) {
          stopPoll();
          await bootstrapAndSeat(server, id, secret, name);
        }
      } catch (e) {
        /* transient — keep polling */
      }
    }, 3000);
  }

  async function bootstrapAndSeat(server, id, secret, name) {
    setStatus("approved — collecting your key…");
    try {
      var r = await fetch(
        server +
          "/join-requests/" +
          encodeURIComponent(id) +
          "/bootstrap?secret=" +
          encodeURIComponent(secret),
        { method: "POST" }
      );
      if (!r.ok) {
        setStatus("could not collect the key: " + (await r.text()));
        return;
      }
      var data = await r.json();
      try {
        sessionStorage.removeItem("loca-join-" + id);
      } catch (e) {}
      // Feed the existing door: the mb_ is a building (Lobby) membership token.
      if ($("name")) $("name").value = name;
      if ($("roomToken")) $("roomToken").value = data.davet;
      if ($("pairingCode")) $("pairingCode").value = "";
      setStatus("joined — taking your seat in the Lobby…");
      if ($("connectBtn")) $("connectBtn").click();
    } catch (e) {
      setStatus("could not reach the server");
    }
  }

  window.addEventListener("load", function () {
    var btn = $("joinRequestBtn");
    if (btn) btn.addEventListener("click", startJoinRequest);
  });
})();
