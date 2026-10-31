// Probe: a form and survey collector. Decides the matrix cells for a
// multi-answer form in one atomic write, tallies, one vote per person, and
// what a duplicate or an invalid answer gets back.
//
//   KONAE_PROBE_PORT=23040 bun run examples/probes/survey/replay.ts
import assert from "node:assert/strict";
import { probe } from "../_kit";

const here = import.meta.dir;

await probe("survey", 23040, async (kit) => {
  const { owner } = kit;
  const app = await kit.publish(here, { projectId: "probe_survey", visibility: "public", signup: { mode: "open" } });
  const submit = (response: string, answers: object | string) => kit.runtime(app.host).call<any>("actions_invoke", {
    name: "survey.submit", input: { response, answers: typeof answers === "string" ? answers : JSON.stringify(answers) },
  });

  // Three answers, one statement: json_each spreads the object into rows.
  assert.equal((await submit("r-1", { role: "designer", team_size: "2-5", comment: "More dark mode" })).changes, 3);
  assert.equal((await submit("r-2", { role: "engineer", team_size: "2-5" })).changes, 2);
  assert.equal((await submit("r-3", { role: "designer", team_size: "6-20", password: "ignored" })).changes, 2);
  // A resubmission with the same key stores nothing; a body that is not an object stores nothing either.
  assert.equal((await submit("r-1", { role: "manager" })).changes, 0);
  assert.equal((await submit("r-4", "[1, 2, 3]")).changes, 0);
  const tally = await kit.runtime(app.host).call<any[]>("actions_invoke", { name: "survey.tally", input: {} });
  assert.deepEqual(tally.map((row) => [row.question, row.answer, row.responses]), [
    ["role", "designer", 2], ["role", "engineer", 1], ["team_size", "2-5", 2], ["team_size", "6-20", 1],
  ]);
  const signals = await owner.call<{ signals: any[] }>("signals_read", { deploymentId: app.deploymentId, name: "survey.submitted" });
  assert.equal(signals.signals.length, 3);
  kit.pass("Multi-answer form", "survey.submit writes every answer of a response in one INSERT ... SELECT FROM json_each(?2), keeps only the three named questions, stores nothing for a repeated response key or a JSON array, and records 3 survey.submitted Signals for the 3 responses that stored rows. survey.tally groups them by question and answer.");

  // An anonymous form cannot tell one person from another; a signed-in vote can.
  const voter = kit.runtime(app.host, await kit.signup(app.host, "vera@probe.example"));
  assert.equal((await voter.call<any>("actions_invoke", { name: "ballots.cast", input: { choice: "tea" } })).changes, 1);
  const twice = await kit.errorText(voter.call("actions_invoke", { name: "ballots.cast", input: { choice: "coffee" } }));
  assert.equal(twice, "You have voted already.");
  const invalid = await kit.errorText(kit.runtime(app.host, await kit.signup(app.host, "otto@probe.example")).call("actions_invoke", { name: "ballots.cast", input: { choice: "juice" } }));
  assert.match(invalid, /CHECK constraint failed/);
  kit.pass("One vote per person", `A signed-in voter's second ballot is refused with "${twice}". An answer outside the allowed values reaches the voter as SQLite's text, "${invalid}", because an input declares a type and a length but no allowed values.`);
});
