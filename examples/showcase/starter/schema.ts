import { index, integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

// The Database Model. schema.sql is what the Kernel runs; keep the two in step.
export const profiles = sqliteTable("profiles", {
  userId: text("user_id").primaryKey(),
  displayName: text("display_name").notNull(),
  role: text("role", { enum: ["member", "admin", "owner"] }).notNull().default("member"),
  avatarFileId: text("avatar_file_id"),
  joinedAt: integer("joined_at").notNull(),
}, (table) => [index("profiles_by_role").on(table.role, table.displayName)]);

export const notes = sqliteTable("notes", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  userId: text("user_id").notNull(),
  title: text("title").notNull(),
  body: text("body").notNull().default(""),
  createdAt: integer("created_at").notNull(),
}, (table) => [index("notes_by_user").on(table.userId, table.id)]);

export const teamPosts = sqliteTable("team_posts", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  authorId: text("author_id").notNull(),
  body: text("body").notNull(),
  createdAt: integer("created_at").notNull(),
});

export const updates = sqliteTable("updates", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  title: text("title").notNull(),
  body: text("body").notNull().default(""),
  source: text("source", { enum: ["team", "program"] }).notNull(),
  authorId: text("author_id").notNull(),
  publishedAt: integer("published_at").notNull(),
});

export const status = sqliteTable("status", {
  component: text("component").primaryKey(),
  state: text("state", { enum: ["operational", "degraded", "down"] }).notNull(),
  detail: text("detail").notNull().default(""),
  updatedAt: integer("updated_at").notNull(),
});

export const escalations = sqliteTable("escalations", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  component: text("component").notNull(),
  changedAt: integer("changed_at").notNull(),
  status: integer("status"),
  filed: integer("filed"),
  error: text("error"),
  answeredAt: integer("answered_at").notNull(),
});

export const attachments = sqliteTable("attachments", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  userId: text("user_id").notNull(),
  fileId: text("file_id").notNull().unique(),
  name: text("name").notNull(),
  size: integer("size").notNull(),
  public: integer("public").notNull().default(0),
  createdAt: integer("created_at").notNull(),
}, (table) => [index("attachments_by_user").on(table.userId, table.id), index("attachments_public").on(table.public, table.id)]);
