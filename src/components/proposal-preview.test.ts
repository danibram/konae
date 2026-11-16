import { describe, expect, test } from "bun:test";
import { textFiles } from "../lib/source-files";
import { cloneStarter } from "../lib/starter";
import { proposalPreviewProject } from "./proposal-preview";

describe("proposal preview", () => {
  test("uses Gate-approved files and retains binary assets and public paths", () => {
    const project = cloneStarter();
    project.files["public/logo.png"] = "iVBORw0KGgo=";
    project.encodings = { "public/logo.png": "base64" };
    project.files["index.html"] = project.files["index.html"].replace("<body>", '<body><img src="/public/logo.png" alt="Logo">');
    const proposed = project.files["index.html"].replace("<h1", '<p>Proposal</p><h1');
    const preview = proposalPreviewProject(project, {
      files: { ...textFiles(project), "index.html": proposed },
      changedFiles: ["index.html"],
      validation: { ok: true, errors: [], checked: ["schema", "actions"] },
    });

    expect(preview?.files["index.html"]).toBe(proposed);
    expect(preview?.files["public/logo.png"]).toBe("iVBORw0KGgo=");
    expect(preview?.encodings).toEqual({ "public/logo.png": "base64" });
  });

  test("does not make an unvalidated proposal previewable", () => {
    const project = cloneStarter();
    expect(proposalPreviewProject(project, {
      files: { ...textFiles(project), "index.html": `${project.files["index.html"]} ` },
      changedFiles: ["index.html"],
      validation: null,
    })).toBeNull();
  });

  test("refuses a proposal after another file changes", () => {
    const project = cloneStarter();
    const proposal = {
      files: { ...textFiles(project), "index.html": `${project.files["index.html"]} ` },
      changedFiles: ["index.html"] as const,
      validation: { ok: true, errors: [], checked: ["schema", "actions"] as ("schema" | "actions")[] },
    };
    const changed = { ...project, files: { ...project.files, "actions.json": `${project.files["actions.json"]} ` } };
    expect(proposalPreviewProject(changed, proposal)).toBeNull();
  });
});
