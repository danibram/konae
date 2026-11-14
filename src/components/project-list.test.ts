import { describe, expect, test } from "bun:test";
import type { DeploymentReceipt } from "../deployments/client";
import { groupProjects, listOrder, nameFromDescription, uniqueName, type ProjectEntry } from "./project-list";

const deployment = (url: string, createdAt: number) => ({ id: `dep_${url}`, projectId: "p", projectName: "p", url, createdAt, manifest: {} }) as unknown as DeploymentReceipt;

const entries: ProjectEntry[] = [
  { id: "a", name: "Reading club", purpose: "Books we read", editedAt: 300, deployment: deployment("http://reading.localhost", 200), inFolder: false },
  { id: "b", name: "Photo wall", purpose: "A public gallery", editedAt: 500, inFolder: false },
  { id: "c", name: "Team journal", purpose: "", deployment: deployment("http://team.localhost", 400), inFolder: true },
  { id: "d", name: "Old folder app", purpose: "", inFolder: true },
];

describe("groupProjects", () => {
  test("an app published from a folder counts as published, beside the ones the editor holds", () => {
    const { published, drafts } = groupProjects(entries, "", "edited");
    expect(published.map((entry) => entry.id)).toEqual(["c", "a"]);
    expect(drafts.map((entry) => entry.id)).toEqual(["b", "d"]);
  });
  test("search matches name, purpose and address, every word", () => {
    expect(groupProjects(entries, "gallery", "edited").drafts.map((entry) => entry.id)).toEqual(["b"]);
    expect(groupProjects(entries, "team.localhost", "edited").published.map((entry) => entry.id)).toEqual(["c"]);
    expect(groupProjects(entries, "photo club", "edited").drafts).toEqual([]);
  });
  test("sort by name ignores case", () => {
    expect(groupProjects(entries, "", "name").drafts.map((entry) => entry.name)).toEqual(["Old folder app", "Photo wall"]);
  });
});

describe("nameFromDescription", () => {
  test("takes the first words of the first sentence", () => {
    expect(nameFromDescription("A sign-up list for Saturday football. People add their name.")).toBe("Sign-up list for Saturday");
    expect(nameFromDescription("  track my plants, and remind me  ")).toBe("Track my plants");
    expect(nameFromDescription("")).toBe("");
    expect(nameFromDescription('A chore rota for "our" flat.')).toBe("Chore rota");
  });
  test("never ends on a small word", () => {
    expect(nameFromDescription("A chore rota for our flat. Each housemate signs in.")).toBe("Chore rota");
    expect(nameFromDescription("A place to log my plants")).toBe("Place to log");
  });
});

describe("uniqueName", () => {
  test("adds a number when a project already has the name", () => {
    expect(uniqueName("Photo wall", ["Reading club"])).toBe("Photo wall");
    expect(uniqueName("Private notes", ["private notes"])).toBe("Private notes 2");
    expect(uniqueName("Private notes", ["Private notes", "Private notes 2"])).toBe("Private notes 3");
  });
});

describe("listOrder", () => {
  test("is the Projects list's order: published first, then drafts, latest on top", () => {
    expect(listOrder(entries).map((entry) => entry.id)).toEqual(["c", "a", "b", "d"]);
  });
});
