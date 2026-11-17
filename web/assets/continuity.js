"use strict";
// Wait and Reminder are core: a loca follows up on declared waits and on an
// enabled silence check whatever its extensions are (the server's sweep runs
// in every loca but a direct message). Their home is the Focus tab, so that
// tab also shows where Focus is off, holding only "Waiting on" and
// "Reminders": when the loca has a wait or the silence check on, and always
// to whoever runs the loca, who sets the rules. The goal is Focus's and the
// tasks the Loca Board's: each reminder rule about them shows only while the
// extension that serves them is on.

// The reminder rules about what an extension serves, by settings key.
const CONTINUITY_RULE_OWNER = { care_goal_secs: "focus", care_task_secs: "board" };

/// Whether a reminder rule applies in this loca: rules about goals and tasks
/// need the extension that serves them.
function reminderRuleServed(key) {
  const owner = CONTINUITY_RULE_OWNER[key];
  return !owner || extensionOn(owner);
}

/// What a reminder's timer can start from here, for the Reminders panel.
function reminderTimerSources() {
  return reminderRuleServed("care_goal_secs") ? "Goal, Task, Wait, or room activity" : "Wait or room activity";
}

/// Whether the Focus tab shows only for Wait and Reminder (Focus is off).
function continuityOnly() {
  if (!state.room || isDm(state.room)) return false;
  // Like every other tab, nothing is shown until the catalog is known.
  if (!extensionCatalogList()) return false;
  const list = currentExtensions();
  return !!list && !list.includes("focus");
}

/// Whether this loca's Focus tab shows although Focus is off.
function continuityTabWanted() {
  if (!continuityOnly()) return false;
  if (isLocaOperator()) return true;
  if (Object.keys(state.waits || {}).length) return true;
  // The wait rule is on by default in every loca, so only the silence check,
  // which an operator turns on, counts as something to show.
  return Number(state.settings?.care_silence_secs || 0) > 0;
}

/// Called by applyLocaExtensions for each tab: true keeps the tab shown.
function continuityShowsTab(key) {
  return key === "tasks" && continuityTabWanted();
}

/// Show or hide what the Focus tab holds for this loca's set.
function refreshContinuityTab() {
  const only = continuityOnly();
  $("focusGoalSection").classList.toggle("hidden", only);
  for (const [checkId, , key] of REMINDER_RULES) {
    $(checkId).closest(".reminderrow")?.classList.toggle("hidden", !reminderRuleServed(key));
  }
  renderFocusBoardLine();
  const tab = $("tabTasks");
  if (!only) return;
  const wanted = continuityTabWanted();
  tab.classList.toggle("hidden", !wanted);
  if (!wanted && state.tab === "tasks") switchTab("chat");
}
