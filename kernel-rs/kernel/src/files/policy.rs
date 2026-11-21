use super::*;

pub fn valid_file_id(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

pub fn file_visibility(file: &Value) -> &'static str {
    if file.get("visibility").and_then(Value::as_str) == Some("public") {
        "public"
    } else {
        "private"
    }
}

pub fn file_owner(file: &Value) -> Option<&str> {
    file.get("ownerId").and_then(Value::as_str)
}

pub fn app_user_files(deployment: &Deployment) -> bool {
    deployment.modules.get("files").is_some_and(|state| state["fileAccess"] == "app-users")
}

pub fn file_readable(deployment: &Deployment, file: &Value, actor_id: Option<&str>) -> bool {
    if app_user_files(deployment) {
        return actor_id.is_some();
    }
    if file_visibility(file) == "public" {
        return !is_private(deployment) || actor_id.is_some();
    }
    actor_id.is_some() && file_owner(file) == actor_id
}

pub fn file_view(deployment: &Deployment, file: &Value, actor_id: Option<&str>) -> Value {
    let mut view = file.clone();
    if let Some(object) = view.as_object_mut() {
        object.remove("url");
        object.remove("sha256");
        if app_user_files(deployment) {
            object.insert("visibility".into(), json!("app-users"));
        }
        if file_owner(file) != actor_id {
            object.remove("ownerId");
        }
    }
    view
}
