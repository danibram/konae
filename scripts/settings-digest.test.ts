import { expect, test } from "bun:test";
import { settingsDigest, settingsMatchAfterLocalModuleDecision } from "./settings-digest";

test("a local legacy module decision does not make settings dirty", () => {
  const remote = { id: "app_1", kernel: "0.8.0", visibility: "public", manifest: {} };
  const local = {
    ...remote,
    modules: {
      files: { on: true, fileAccess: null },
      seo: { on: false, canonical: null, image: null },
    },
  };
  expect(settingsMatchAfterLocalModuleDecision(local, remote)).toBe(true);
});

test("local auth decisions on and off do not make settings dirty", () => {
  const remote = { id: "app_1", kernel: "0.8.0", visibility: "public", manifest: {} };
  for (const on of [true, false]) {
    const local = {
      ...remote,
      modules: {
        auth: { on },
        files: { on: true, fileAccess: null },
        seo: { on: false, canonical: null, image: null },
      },
    };
    expect(settingsMatchAfterLocalModuleDecision(local, remote)).toBe(true);
  }
});

test("a hand edit to settings remains dirty after a local module decision", () => {
  const remote = { id: "app_1", kernel: "0.8.0", visibility: "public", manifest: {} };
  const local = {
    ...remote,
    contactEmail: "owner@example.test",
    modules: {
      files: { on: true, fileAccess: null },
      seo: { on: false, canonical: null, image: null },
    },
  };
  expect(settingsMatchAfterLocalModuleDecision(local, remote)).toBe(false);
  expect(settingsDigest(local)).not.toBe(settingsDigest(remote));
});

test("a hand-edited module state remains dirty", () => {
  const remote = { id: "app_1", kernel: "0.8.0", visibility: "public", manifest: {} };
  const local = { ...remote, modules: { files: { on: true, fileAccess: null }, seo: { on: true, canonical: null, image: null } } };
  expect(settingsMatchAfterLocalModuleDecision(local, remote)).toBe(false);
});

test("a hand edit to an auth option remains dirty", () => {
  const remote = { id: "app_1", kernel: "0.8.0", visibility: "public", manifest: {} };
  const local = {
    ...remote,
    modules: {
      auth: { on: true, signupPolicy: { mode: "closed", allowlist: [] } },
      files: { on: true, fileAccess: null },
      seo: { on: false, canonical: null, image: null },
    },
  };
  expect(settingsMatchAfterLocalModuleDecision(local, remote)).toBe(false);
});
