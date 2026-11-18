use super::{hex_lower, store, PathScope, Repository, Reservation};
use anyhow::{ensure, Context, Result};
use std::collections::{HashMap, HashSet};

struct Plan {
    points: Vec<String>,
    objects: Vec<String>,
    abandoned: Vec<String>,
}

impl Repository<'_> {
    /// Collect garbage from one complete inventory under repository exclusion.
    /// A caller pruning after publication supplies the ID it just committed.
    pub async fn prune(&self, scope: &dyn PathScope, now: u64, current: &str) -> Result<usize> {
        let guard = Reservation::acquire(self.source, true).await?;
        let plan = self.mark(scope, now, current).await;
        let result = match plan {
            Ok(plan) => {
                #[cfg(test)]
                super::tests::checkpoint("mark");
                // Keep an S3 reservation on an unconfirmed DELETE. Recovery
                // fences the failed collector before admitting another writer.
                for id in plan.points.iter().chain(&plan.abandoned) {
                    store::remove(self.source, "points", id).await?;
                }
                if !plan.points.is_empty() || !plan.abandoned.is_empty() {
                    store::sync_group(self.source, "points")?;
                }
                for id in &plan.objects {
                    store::remove(self.source, "objects", id).await?;
                }
                if !plan.objects.is_empty() {
                    store::sync_group(self.source, "objects")?;
                }
                Ok(plan.points.len())
            }
            Err(error) => Err(error),
        };
        guard.release().await?;
        result
    }

    async fn mark(&self, scope: &dyn PathScope, now: u64, current: &str) -> Result<Plan> {
        let store::Roots { points, abandoned } = store::roots(self.source).await?;
        ensure!(
            points.iter().any(|id| id == current),
            "The destination omitted the published point."
        );
        let mut expired: HashSet<_> = crate::expired_copies(&points, now).into_iter().collect();
        expired.remove(current);
        let mut reachable = HashMap::new();
        for id in &points {
            let point = self
                .read_point(id, scope)
                .await
                .with_context(|| format!("Cannot prune with unreadable point {id}"))?;
            if expired.contains(id) {
                continue;
            }
            for (address, size) in point.object_sizes() {
                ensure!(
                    reachable
                        .insert(address.to_owned(), size)
                        .is_none_or(|old| old == size),
                    "Retained points disagree on a block length."
                );
            }
        }

        let mut objects = Vec::new();
        for entry in store::list(self.source, "objects").await? {
            ensure!(
                hex_lower(&entry.name, 64) || store::temporary(self.source, &entry.name),
                "Unknown v3 object name."
            );
            if let Some(size) = reachable.remove(&entry.name) {
                ensure!(entry.size == size, "Retained block has the wrong length.");
            } else if now.saturating_sub(entry.modified_ms) >= crate::DAY_MS {
                objects.push(entry.name);
            }
        }
        ensure!(
            reachable.is_empty(),
            "The destination omitted {} retained blocks.",
            reachable.len()
        );
        Ok(Plan {
            points: expired.into_iter().collect(),
            objects,
            abandoned,
        })
    }
}
