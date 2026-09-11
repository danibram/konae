import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { withPrivateNotes } from "../lib/private-notes";
import { MiniAppRuntimeHost } from "./host";

const localValues = new Map<string, string>();
const previousLocalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");

beforeEach(() => {
  localValues.clear();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => localValues.get(key) ?? null,
      setItem: (key: string, value: string) => { localValues.set(key, value); },
      removeItem: (key: string) => { localValues.delete(key); },
    },
  });
});

afterEach(() => {
  if (previousLocalStorage) Object.defineProperty(globalThis, "localStorage", previousLocalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

describe("private notes preview auth", () => {
  test("keeps Alice and Bob notes separate across sign-in and database reload", async () => {
    const project = withPrivateNotes(cloneStarter());
    const host = new MiniAppRuntimeHost(project.id, project.files["schema.sql"], project.files["actions.json"]);

    await expect(host.handle("actions.invoke", { name: "notes.list" })).rejects.toThrow("authentication-required");
    const alice = await host.handle("auth.signIn", {}) as { userId: string; name: string; role: string };
    expect(alice).toMatchObject({ name: "Alice", role: "member" });
    await host.handle("actions.invoke", { name: "notes.create", input: { title: "Alice's note" } });

    host.setPreviewActor("bob");
    const bobBeforeCreate = await host.handle("actions.invoke", { name: "notes.list" });
    expect(bobBeforeCreate).toEqual([]);
    await host.handle("actions.invoke", { name: "notes.create", input: { title: "Bob's note" } });

    host.setPreviewActor("alice");
    expect(await host.handle("actions.invoke", { name: "notes.list" })).toMatchObject([
      { title: "Alice's note" },
    ]);

    const reloadedHost = new MiniAppRuntimeHost(project.id, project.files["schema.sql"], project.files["actions.json"]);
    reloadedHost.setPreviewActor("alice");
    expect(await reloadedHost.handle("actions.invoke", { name: "notes.list" })).toMatchObject([
      { title: "Alice's note" },
    ]);
    reloadedHost.setPreviewActor("bob");
    expect(await reloadedHost.handle("actions.invoke", { name: "notes.list" })).toMatchObject([
      { title: "Bob's note" },
    ]);
  });

  test("notifies the Auth panel when app sign-in and sign-out change the preview actor", async () => {
    const project = withPrivateNotes(cloneStarter());
    const host = new MiniAppRuntimeHost(project.id, project.files["schema.sql"], project.files["actions.json"]);
    const observed: string[] = [];
    const unsubscribe = host.subscribeSession((session) => observed.push(session.name));

    await host.handle("auth.signIn", {});
    await host.handle("auth.signOut", {});
    unsubscribe();

    expect(observed).toEqual(["Anonymous", "Alice", "Anonymous"]);
    expect(host.getPreviewActor()).toBe("guest");
  });
});
