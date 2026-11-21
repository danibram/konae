import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

const ownerLogin = readFileSync(new URL("../kernel-rs/konae/src/owners/login.html", import.meta.url), "utf8");

function section(start: string, end: string) {
  const from = ownerLogin.indexOf(start);
  const to = ownerLogin.indexOf(end, from);
  expect(from).toBeGreaterThanOrEqual(0);
  expect(to).toBeGreaterThan(from);
  return ownerLogin.slice(from, to);
}

test("same-tab verification keeps signup state but validates the new password", async () => {
  const password = { value: "x" };
  const hintMessages: string[] = [];
  const requests: Array<{ path: string; body: Record<string, string> }> = [];
  let currentView = "#auth";
  const answerForm = {
    id: "answer",
    querySelector(selector: string) {
      return selector.includes("email") ? null : password;
    },
    onsubmit: undefined as ((event: { preventDefault(): void; currentTarget: unknown }) => Promise<void>) | undefined,
  };
  const context = {
    signup: true,
    emailVerificationRequired: true,
    linkPurpose: null as string | null,
    linkToken: null as string | null,
    location: { hash: "#verification=verify-token", pathname: "/_konae/owner/login", search: "" },
    history: { replaceState() {} },
    URLSearchParams,
    window: {},
    document: { querySelector() { return null; } },
    show(view: string) { currentView = view; },
    $: (selector: string) => selector === "#answer"
      ? answerForm
      : { closest: () => ({ firstChild: { textContent: "" } }) },
    invalid(_input: unknown, message: string) { hintMessages.push(message); },
    busy() {},
    async post(path: string, body: Record<string, string>) {
      requests.push({ path, body });
      return { status: 200, body: {} };
    },
    async session() { return { signedIn: true }; },
    async signedIn() {},
    FormData: class {
      constructor(_form: unknown) {}
      get(name: string) { return name === "password" ? password.value : null; }
    },
  };

  runInNewContext([
    section("function readLink(){", "window.addEventListener('hashchange',readLink);"),
    section("function validate(", "$('#mode').onclick="),
    section("$('#answer').onsubmit=", "$('#new-link').onclick="),
  ].join("\n"), context);

  expect(runInNewContext("readLink()", context)).toBe(true);
  expect(context.signup).toBe(true);
  expect(currentView).toBe("#answer");
  const signupAuthForm = {
    id: "auth",
    querySelector(selector: string) { return selector.includes("email") ? null : password; },
  };
  expect(runInNewContext("validate(signupAuthForm, false, false)", { ...context, signupAuthForm })).toBe(true);

  await answerForm.onsubmit!({ preventDefault() {}, currentTarget: answerForm });
  expect(hintMessages).toEqual(["Use at least 8 characters."]);
  expect(currentView).toBe("#answer");
  expect(requests).toEqual([]);

  password.value = "valid-test-password";
  await answerForm.onsubmit!({ preventDefault() {}, currentTarget: answerForm });
  expect(requests).toEqual([{ path: "/_konae/owner/verify", body: { token: "verify-token", password: "valid-test-password" } }]);
});

test("optional-verification signup still requires a password", () => {
  const messages: string[] = [];
  const signupForm = {
    id: "auth",
    querySelector(selector: string) { return selector.includes("email") ? null : { value: "x" }; },
  };
  const context = { signup: true, emailVerificationRequired: false, invalid(_input: unknown, message: string) { messages.push(message); } };
  runInNewContext(section("function validate(", "$('#mode').onclick="), context);

  expect(runInNewContext("validate(signupForm, false, false)", { ...context, signupForm })).toBe(false);
  expect(messages).toEqual(["Use at least 8 characters."]);
});
