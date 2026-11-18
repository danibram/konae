import { describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { registerStorageConformance } from "@earendil-works/pi-durable/testing";
import { IndexedDbStorage } from "@earendil-works/pi-durable/storage/indexeddb";
import { ROOT_CONVERSATION_ID, type Storage } from "@earendil-works/pi-durable";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";

Object.assign(globalThis, { IDBKeyRange });

function fixtureStorage() {
  const factory = new IDBFactory();
  let loseNextAck = false;
  const open = async () => IndexedDbStorage.open("durable-spike", { indexedDB: factory });
  return {
    async create() {
      const storage = await open();
      const proxy = new Proxy(storage, {
        get(target, property) {
          if (property === "commit") return async (...args: Parameters<Storage["commit"]>) => {
            const result = await target.commit(...args);
            if (loseNextAck) {
              loseNextAck = false;
              throw new Error("fixture: durable commit completed; acknowledgement lost");
            }
            return result;
          };
          const value = Reflect.get(target, property, target);
          return typeof value === "function" ? value.bind(target) : value;
        },
      }) as Storage;
      return { storage: proxy, close: () => storage.close(BACKGROUND_CONTEXT), loseAck: () => { loseNextAck = true; }, open };
    },
  };
}

registerStorageConformance({ describe, expect, it }, "IndexedDB with lost-receipt fault hook", async (use) => {
  const adapter = fixtureStorage();
  const opened = await adapter.create();
  try { await use(opened.storage); }
  finally { await opened.close(); }
});

it("fault hook reports unknown acknowledgement while committed row survives reopen", async () => {
  const adapter = fixtureStorage();
  const first = await adapter.create();
  first.loseAck();
  await expect(first.storage.commit(
    [[{ type: "conversation", value: { id: ROOT_CONVERSATION_ID } }]],
    () => {},
    BACKGROUND_CONTEXT,
  )).rejects.toThrow("acknowledgement lost");
  await first.close();

  const second = await adapter.create();
  try {
    expect(await second.storage.conversation(ROOT_CONVERSATION_ID, BACKGROUND_CONTEXT)).toEqual({ id: ROOT_CONVERSATION_ID });
  } finally { await second.close(); }
});
