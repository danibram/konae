use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::{
    ActionProgram, AppState, Ran, action_lock, authenticate_deployment, calls, copies,
    deployment_for, enter, error, guard, http, is_private, modules, notify, now_ms, refusal_error,
    resolve_bindings, run_action_in_slot, usage,
};

pub async fn invoke_action(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Response {
    let mut response = invoke_action_inner(state, name, headers.clone(), input).await;
    crate::functions::trace::attach_request_response(&headers, &mut response);
    response
}

async fn invoke_action_inner(
    state: AppState,
    name: String,
    headers: HeaderMap,
    input: Value,
) -> Response {
    let deployment = match deployment_for(&state, &headers).await {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Deployment not found."),
    };
    if let Err(refused) = modules::require_on(&deployment.modules, "actions") {
        return refusal_error(&refused);
    }
    let program: ActionProgram = match serde_json::from_str(&deployment.actions) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stored action program is invalid.",
            );
        }
    };
    let action = match program.actions.get(&name) {
        Some(value) => value,
        None => return error(StatusCode::NOT_FOUND, "Server action not declared."),
    };
    if action.access == "scheduled" {
        return error(StatusCode::FORBIDDEN, &format!("Action {name} runs only inside a scheduled Function."));
    }
    // An action whose route does not answer anyone answers only there.
    if let Some(route) = http::route_only(&name, action, is_private(&deployment)) {
        return error(
            StatusCode::FORBIDDEN,
            &format!("Action {name} answers only on its HTTP route, {route}."),
        );
    }
    // A call's then mutation records what the call's answer said, so no one
    // else may write it.
    if let Some(call) = calls::then_of(&program, &name) {
        return error(
            StatusCode::FORBIDDEN,
            &format!("Action {name} runs only after the call {call} answers."),
        );
    }
    // A scheduled mutation with no route belongs to the Kernel's timer

    if action.schedule.is_some() && action.http.is_none() {
        return error(
            StatusCode::FORBIDDEN,
            &format!("Action {name} runs only on its schedule."),
        );
    }
    let actor = match authenticate_deployment(&state, &deployment, &headers).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    if action.access == "granted" {
        return error(StatusCode::FORBIDDEN, "action-grant-required");
    }

    if (action.access != "anonymous" || is_private(&deployment)) && actor.is_none() {
        return error(StatusCode::UNAUTHORIZED, "authentication-required");
    }
    let params = match resolve_bindings(
        action,
        &input,
        actor.as_ref().map(|user| user.user_id.as_str()),
    ) {
        Ok(value) => value,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };

    if action.kind == "call" {
        let traceparent = state.otlp.as_ref().and_then(|_| (headers.get_all(crate::functions::trace::TRACEPARENT).iter().count() == 1)
            .then(|| headers.get(crate::functions::trace::TRACEPARENT).and_then(|value| value.to_str().ok()))
            .flatten());
        let response = calls::enqueue_with_traceparent(&state, &deployment, &name, &input, traceparent).await;
        if response.status().is_success() && !enter::entered(actor.as_ref()) {
            if let Some(user) = actor.as_ref() {
                usage::count_account(
                    &state.data_root,
                    &deployment.id,
                    &user.user_id,
                    usage::AccountKind::Action,
                );
            }
        }
        return response;
    }
    let db_path = crate::layout::database(&state.data_root, &deployment.id);

    copies::operation::action(&state, &deployment.id, &name, action, async {
        let result = if action.kind == "signal" {
            Ok(Ran {
                answer: json!({"recorded": true}),
                rows: Vec::new(),
                sqlite: std::time::Duration::ZERO,
            })
        } else {
            let _guard = action_lock(&state, &deployment.id, &action.kind).await;
            match state.guards.statement_permit(&deployment.id).await {
                Ok(permit) => run_action_in_slot(db_path, action, params, permit).await,
                Err(wait) => return http::rate_limited(wait),
            }
        };
        let actor_id = actor.as_ref().map(|user| user.user_id.as_str());

        let counted = !enter::entered(actor.as_ref());
        match result {
            Ok(ran) => match guard::outcome(&name, action, &ran.answer) {
                guard::Outcome::Refused(message) => {
                    if counted {
                        usage::execution(
                            &state.data_root,
                            &deployment.id,
                            &name,
                            "runtime",
                            "refused",
                        );
                        usage::count(
                            &state.data_root,
                            &deployment.id,
                            usage::Operation::Refused(name.clone()),
                        );
                    }
                    error(StatusCode::FORBIDDEN, &message)
                }
                changed => {
                    if counted {
                        usage::execution(&state.data_root, &deployment.id, &name, "runtime", "ok");
                        usage::count(
                            &state.data_root,
                            &deployment.id,
                            usage::Operation::Action(name.clone()),
                        );
                        if let Some(account) = actor_id {
                            usage::count_account(
                                &state.data_root,
                                &deployment.id,
                                account,
                                usage::AccountKind::Action,
                            );
                        }
                    }
                    if matches!(changed, guard::Outcome::Changed) {
                        if counted {
                            usage::signal(
                                &state.data_root,
                                &deployment.id,
                                &name,
                                action,
                                &input,
                                actor_id,
                            );
                        }
                        let caller = notify::Caller {
                            account: actor_id,
                            actor_id,
                        };
                        notify::record(
                            &state,
                            &deployment,
                            &name,
                            action,
                            &input,
                            caller,
                            ran.changed(now_ms()),
                        )
                        .await;
                    }
                    Json(json!({"result": ran.answer})).into_response()
                }
            },
            Err(error_value) => {
                if counted {
                    usage::execution(&state.data_root, &deployment.id, &name, "runtime", "failed");
                }
                error(StatusCode::BAD_REQUEST, &error_value.to_string())
            }
        }
    })
    .await
}
