import { index, integer, sqliteTable, text, unique } from "drizzle-orm/sqlite-core";

// The Database Model. schema.sql is what the Kernel runs; keep the two in step.
export const habits = sqliteTable("habits", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  userId: text("user_id").notNull(),
  name: text("name").notNull(),
  createdAt: integer("created_at").notNull(),
  archivedAt: integer("archived_at"),
}, (table) => [index("habits_by_user").on(table.userId, table.archivedAt)]);

export const checkins = sqliteTable("checkins", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  habitId: integer("habit_id").notNull().references(() => habits.id),
  userId: text("user_id").notNull(),
  day: text("day").notNull(),
  device: text("device").notNull(),
  createdAt: integer("created_at").notNull(),
}, (table) => [unique().on(table.habitId, table.day), index("checkins_by_user").on(table.userId, table.day)]);
