// Deleting a closed loca, the Master's: its records go from the server, with
// or without a zip of them first. Seal (api.js deleteRoom) keeps them. The
// zip is fetched with the session and handed over as an object URL the page
// made, as an attachment is (chat.js saveAttachment), so the desktop window
// saves it to Downloads too. Nothing is deleted when the zip did not arrive.

let locaDeleteTarget = null;

function locaDeleteButtons() {
  return ["locaDeleteOnly", "locaDeleteArchive"].map(id => $(id));
}

function renderLocaDeleteButtons(busy = false) {
  const matches = $("locaDeleteName").value === locaDeleteTarget;
  for (const button of locaDeleteButtons()) button.disabled = busy || !matches;
  $("locaDeleteCancel").disabled = busy;
}

function openLocaDelete(room) {
  locaDeleteTarget = room;
  $("locaDeleteRoom").textContent = room;
  $("locaDeleteName").value = "";
  $("locaDeleteStatus").textContent = "";
  $("locaDeleteError").textContent = "";
  renderLocaDeleteButtons();
  openExtDialog("locaDelete", "locaDeleteName");
}

function closeLocaDelete() {
  if ($("locaDeleteCancel").disabled) return;
  locaDeleteTarget = null;
  closeExtDialog("locaDelete");
}

async function downloadLocaArchive(room) {
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/archive`, {
    headers: adminHeaders({}),
  });
  if (!r.ok) throw new Error((await r.text()) || `archive ${r.status}`);
  const named = /filename="([^"]+)"/.exec(r.headers.get("content-disposition") || "");
  const url = URL.createObjectURL(await r.blob());
  const link = document.createElement("a");
  link.href = url;
  link.download = named ? named[1] : "loca-archive.zip";
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}

async function runLocaDelete(withArchive) {
  const room = locaDeleteTarget;
  if (!room || $("locaDeleteName").value !== room) return;
  $("locaDeleteError").textContent = "";
  renderLocaDeleteButtons(true);
  try {
    if (withArchive) {
      $("locaDeleteStatus").textContent = "preparing the zip…";
      try {
        await downloadLocaArchive(room);
      } catch (error) {
        $("locaDeleteError").textContent = `the zip could not be made — nothing was deleted (${error.message})`;
        return;
      }
      $("locaDeleteStatus").textContent = "zip saved; deleting…";
    } else {
      $("locaDeleteStatus").textContent = "deleting…";
    }
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/records`, {
      method: "DELETE", headers: adminHeaders({}),
    });
    if (r.status !== 204) {
      $("locaDeleteError").textContent = (await r.text()) || `could not delete (${r.status})`;
      $("locaDeleteStatus").textContent = "";
      return;
    }
    renderLocaDeleteButtons(false);
    closeLocaDelete();
    if (state.room === room) leaveFor(room, state.homeRoom);
    refreshRooms();
  } catch {
    $("locaDeleteError").textContent = "the server could not be reached — try again";
    $("locaDeleteStatus").textContent = "";
  } finally {
    if (locaDeleteTarget) renderLocaDeleteButtons(false);
  }
}

$("deleteLocaBtn").onclick = () => openLocaDelete(state.room);
$("locaDeleteName").addEventListener("input", () => renderLocaDeleteButtons());
$("locaDeleteOnly").onclick = () => runLocaDelete(false);
$("locaDeleteArchive").onclick = () => runLocaDelete(true);
$("locaDeleteCancel").onclick = closeLocaDelete;
$("locaDeleteBackdrop").onclick = closeLocaDelete;
$("locaDelete").addEventListener("keydown", event => {
  if (event.key === "Escape") closeLocaDelete();
});
