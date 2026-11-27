use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
};

use axum::{body::Body, extract::Request};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::{
    AppState, Deployment, Operation,
    refusal::{Refusal, refuse},
    usage,
};

use super::{
    Function,
    http::{CronSkipped, ResponseCode, Trigger, invoke},
};

#[cfg(test)]
#[path = "cron_tests.rs"]
mod tests;

pub const MINUTE_MS: u64 = 60_000;
const FIELDS: [(u8, u8); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cron {
    pub cron: Expression,
    pub function: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct Expression {
    text: String,
    masks: [u64; 5],
    both_days_restricted: bool,
}

impl Expression {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn matches(&self, time: OffsetDateTime) -> bool {
        let [minutes, hours, days, months, weekdays] = self.masks;
        let day = days & (1 << time.day()) != 0;
        let weekday = weekdays & (1 << time.weekday().number_days_from_sunday()) != 0;
        let calendar_day = if self.both_days_restricted {
            day || weekday
        } else {
            day && weekday
        };
        calendar_day
            && months & (1 << u8::from(time.month())) != 0
            && hours & (1 << time.hour()) != 0
            && minutes & (1 << time.minute()) != 0
    }
}

impl TryFrom<String> for Expression {
    type Error = &'static str;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        const INVALID: &str = "Invalid five-field cron expression.";
        if text.len() > 128 || !text.is_ascii() {
            return Err(INVALID);
        }
        let fields: Vec<_> = text.split_ascii_whitespace().collect();
        if fields.len() != FIELDS.len() {
            return Err(INVALID);
        }
        let mut masks = [0; 5];
        for (index, (field, (min, max))) in fields.iter().zip(FIELDS).enumerate() {
            masks[index] = field_mask(field, min, max).ok_or(INVALID)?;
        }
        let both_days_restricted = !fields[2].starts_with('*') && !fields[4].starts_with('*');
        if masks[4] & (1 << 7) != 0 {
            masks[4] |= 1;
        }
        Ok(Self {
            text: fields.join(" "),
            masks,
            both_days_restricted,
        })
    }
}

impl From<Expression> for String {
    fn from(value: Expression) -> Self {
        value.text
    }
}

fn number(value: &str) -> Option<u8> {
    if !(1..=2).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn field_mask(field: &str, min: u8, max: u8) -> Option<u64> {
    let mut mask = 0;
    for item in field.split(',') {
        let (base, step) = match item.split_once('/') {
            Some((base, step)) if base == "*" || base.contains('-') => (base, number(step)?),
            Some(_) => return None,
            None => (item, 1),
        };
        let (first, last) = match base {
            "*" => (min, max),
            _ => match base.split_once('-') {
                Some((first, last)) => (number(first)?, number(last)?),
                None => {
                    let value = number(base)?;
                    (value, value)
                }
            },
        };
        if first < min || last > max || first > last || step == 0 || step > max - min + 1 {
            return None;
        }
        for value in (first..=last).step_by(usize::from(step)) {
            mask |= 1 << value;
        }
    }
    Some(mask)
}

type Declarations = BTreeMap<String, Cron>;

pub fn read(
    source: &str,
    functions: Option<&BTreeMap<String, Function>>,
) -> Result<Declarations, Refusal> {
    let invalid = || {
        refuse(
            "cron.invalid",
            "crons.json must be a JSON object with version 1 and a crons object.",
        )
    };
    let source: Value = serde_json::from_str(source).map_err(|_| invalid())?;
    let entries = source["crons"]
        .as_object()
        .filter(|_| {
            source["version"].as_f64() == Some(1.0)
                && source.as_object().is_some_and(|object| object.len() == 2)
        })
        .ok_or_else(invalid)?;
    if entries.len() > 32 {
        return Err(refuse(
            "cron.count-over",
            "crons.json may declare at most 32 jobs.",
        ));
    }
    entries.iter().map(|(id, entry)| {
        if !super::identifier(id) {
            return Err(refuse("cron.id-invalid", format!("Cron {id} must have an ID of 1 to 64 ASCII letters, digits, underscores or hyphens, starting with a letter.")));
        }
        let Some((cron, function)) = entry["cron"].as_str().zip(entry["function"].as_str())
            .filter(|_| entry.as_object().is_some_and(|object| object.len() == 2))
        else {
            return Err(refuse("cron.definition-invalid", format!("Cron {id} must declare a cron string and a function name.")));
        };
        let cron = Expression::try_from(cron.to_owned()).map_err(|_| refuse("cron.expression-invalid", format!("Cron {id} must use a five-field UTC cron expression, such as \"0 8 * * 1-5\".")))?;
        let target = functions.and_then(|functions| functions.get(function));
        if target.is_none_or(|target| target.access != "scheduled") {
            return Err(refuse("cron.target-invalid", format!("Cron {id} must name a Function with access scheduled in functions.json. Check {function}.")));
        }
        Ok((id.clone(), Cron { cron, function: function.to_owned() }))
    }).collect()
}

pub(crate) fn run_name(id: &str) -> String {
    format!("cron:{id}")
}

pub(crate) fn slot(at: u64) -> i64 {
    (at / MINUTE_MS) as i64
}

pub(crate) fn released_before_slot(deployment: &Deployment, at: u64) -> bool {
    slot(deployment.released_at) < slot(at)
}

pub(crate) fn declarations(deployment: &Deployment) -> Option<&Declarations> {
    deployment
        .server
        .as_ref()
        .map(|server| server.manifest.crons.as_ref())
        .filter(|crons| !crons.is_empty())
}

pub(crate) fn tick(
    state: &AppState,
    deployment: &Deployment,
    running: &Arc<Mutex<HashSet<String>>>,
    at: u64,
    time: OffsetDateTime,
) {
    let Some(crons) = declarations(deployment) else {
        return;
    };
    if !released_before_slot(deployment, at) {
        return;
    }
    let due = crons
        .iter()
        .filter(|(_, job)| job.cron.matches(time))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    if due.is_empty()
        || !running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(deployment.id.clone())
    {
        return;
    }
    let (state, deployment, running) = (state.clone(), deployment.clone(), running.clone());
    tokio::spawn(async move {
        for id in due {
            if let Err(error) = run(&state, &deployment, &id, at, false).await {
                eprintln!(
                    "Cron {id} on Deployment {} failed. {error:#}",
                    deployment.id
                );
            }
        }
        running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&deployment.id);
    });
}

pub(crate) async fn run(
    state: &AppState,
    deployment: &Deployment,
    id: &str,
    at: u64,
    manual: bool,
) -> anyhow::Result<Value> {
    let cron = declarations(deployment)
        .and_then(|crons| crons.get(id))
        .ok_or_else(|| anyhow::anyhow!("Cron {id} is not declared."))?;
    let scheduled_at = if manual { at } else { at - at % MINUTE_MS };
    let key = if manual {
        format!("cron:{id}:test:{}", uuid::Uuid::new_v4())
    } else {
        format!("cron:{id}:{}", slot(at))
    };
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/?cron={id}&scheduledAt={scheduled_at}"))
        .header("idempotency-key", key)
        .body(Body::empty())?;
    let operation = Operation(Arc::new(state.copies.operations.clone().read_owned().await));
    request.extensions_mut().insert(operation.clone());
    let response = invoke(
        state,
        deployment,
        cron.function.clone(),
        request,
        Trigger::Cron { id, at, manual },
    )
    .await
    .map_err(|_| anyhow::anyhow!("Cron {id} could not select a Function."))?;
    if response.extensions().get::<CronSkipped>().is_some() {
        anyhow::ensure!(
            !manual,
            "Cron {id} could not run while the Deployment changed or was paused."
        );
        return Ok(json!({"ran": false}));
    }
    if response
        .extensions()
        .get::<ResponseCode>()
        .is_some_and(|code| code.0 == "cron.claim-failed")
    {
        anyhow::bail!("The Kernel could not record the cron attempt.");
    }
    let status = response.status().as_u16();
    let at = crate::now_ms();
    let folder = crate::layout::data(&state.data_root, &deployment.id);
    let name = run_name(id);
    let schedule = cron.cron.text().to_owned();
    let error = (status >= 400).then(
        || match response.extensions().get::<ResponseCode>() {
            Some(code) => format!("Function failed with {} ({status}).", code.0),
            None => format!("Function returned status {status}."),
        },
    );
    let recorded_error = error.clone();
    tokio::task::spawn_blocking(move || {
        let _operation = operation;
        usage::record_cron(
            &folder,
            &usage::Run {
                action: &name,
                schedule: &schedule,
                slot: slot(scheduled_at),
                at,
                changes: recorded_error.is_none().then_some(0),
                error: recorded_error,
            },
        )
    })
    .await??;
    if let Some(error) = error {
        anyhow::bail!("{error}");
    }
    Ok(
        json!({"ran": true, "cron": id, "function": cron.function, "scheduledAt": scheduled_at, "status": status, "at": at}),
    )
}
