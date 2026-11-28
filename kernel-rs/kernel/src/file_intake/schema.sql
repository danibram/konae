CREATE TABLE IF NOT EXISTS __konae_intake_versions (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  source_version INTEGER NOT NULL,
  path TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  size INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS __konae_intake_versions_path ON __konae_intake_versions(source_id,path,created_at);
CREATE TABLE IF NOT EXISTS __konae_intake_deliveries (
  source_id TEXT NOT NULL,
  delivery_key TEXT NOT NULL,
  version_id TEXT NOT NULL REFERENCES __konae_intake_versions(id),
  sha256 TEXT NOT NULL,
  path TEXT NOT NULL,
  PRIMARY KEY(source_id,delivery_key)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS __konae_intake_current (
  source_id TEXT NOT NULL,
  path TEXT NOT NULL,
  version_id TEXT NOT NULL REFERENCES __konae_intake_versions(id),
  sha256 TEXT NOT NULL,
  PRIMARY KEY(source_id,path)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS __konae_intake_jobs (
  id TEXT PRIMARY KEY,
  version_id TEXT NOT NULL REFERENCES __konae_intake_versions(id),
  consumer_id TEXT NOT NULL,
  action TEXT NOT NULL,
  input TEXT NOT NULL,
  action_json TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('pending','leased','done','failed')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  leased_until INTEGER,
  created_at INTEGER NOT NULL,
  UNIQUE(version_id,consumer_id)
);
CREATE INDEX IF NOT EXISTS __konae_intake_jobs_due ON __konae_intake_jobs(state,next_attempt_at);
CREATE TABLE IF NOT EXISTS __konae_intake_effects (
  version_id TEXT NOT NULL,
  consumer_id TEXT NOT NULL,
  job_id TEXT NOT NULL UNIQUE,
  committed_at INTEGER NOT NULL,
  PRIMARY KEY(version_id,consumer_id)
) WITHOUT ROWID;
