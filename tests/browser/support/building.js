const { expect } = require("@playwright/test");

// Test rooms are provisioned explicitly through the same creation endpoint as
// the UI. Connection, invitation and message helpers must never create them.
async function provisionRooms(request, names) {
  for (const name of names) {
    const response = await request.post("/rooms", {
      headers: { "x-admin-token": "MASTER" }, data: { name },
    });
    expect([201, 409]).toContain(response.status());
    expect((await request.get(`/rooms/${encodeURIComponent(name)}/settings`, {
      headers: { "x-admin-token": "MASTER" },
    })).ok()).toBeTruthy();
  }
}
module.exports = { provisionRooms };
