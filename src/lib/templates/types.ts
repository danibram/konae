export type TemplateFile = "index.html" | "styles.css" | "app.js" | "schema.sql" | "actions.json";

/** A complete Capsule a project can be born as, with the Drizzle model that mirrors its schema.sql. */
export type TemplateSource = {
  id: string;
  name: string;
  /** One sentence the new-project dialog shows under the name. */
  sentence: string;
  model: string;
  files: Record<TemplateFile, string>;
};
