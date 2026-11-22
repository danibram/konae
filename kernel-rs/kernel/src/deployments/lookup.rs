use std::collections::HashMap;

use crate::{AppState, Deployment};

pub fn find_project_deployment(
    deployments: &HashMap<String, Deployment>,
    project_id: &str,
) -> Option<Deployment> {
    deployments
        .values()
        .find(|deployment| deployment.project_id == project_id)
        .cloned()
}

pub async fn deployment_by_id(state: &AppState, id: &str) -> Option<Deployment> {
    state.deployments.read().await.values().find(|deployment| deployment.id == id).cloned()
}
