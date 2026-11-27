use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::calls;

#[derive(Deserialize)]
pub struct ActionProgram {
    pub version: u8,
    pub actions: HashMap<String, ActionDefinition>,
}

#[derive(Clone, Deserialize)]
pub struct ActionDefinition {
    pub kind: String,
    pub access: String,

    #[serde(default)]
    pub statement: String,
    pub bindings: Vec<ActionBinding>,

    #[serde(default)]
    pub http: Option<Value>,

    #[serde(default)]
    pub page: Option<Value>,

    #[serde(default)]
    pub signal: Option<Value>,

    #[serde(default)]
    pub guard: Option<Value>,

    #[serde(default)]
    pub notify: Option<Value>,

    #[serde(default)]
    pub schedule: Option<Value>,

    #[serde(default)]
    pub issue: Option<crate::documents::IssueDeclaration>,

    #[serde(default)]
    pub subscribe: Option<Value>,


    #[serde(flatten)]
    pub call: calls::CallKeys,


    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBinding {
    pub source: String,
    pub key: Option<String>,
    #[serde(rename = "type")]
    pub value_type: Option<String>,
    #[serde(default, deserialize_with = "whole_number")]
    pub max_length: Option<usize>,
    #[serde(default)]
    pub nullable: bool,
    pub field: Option<String>,
    pub value: Option<Value>,
}



fn whole_number<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<usize>, D::Error> {
    Option::<f64>::deserialize(deserializer)?
        .map(|number| {
            (number.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&number))
                .then_some(number as usize)
                .ok_or_else(|| serde::de::Error::custom("expected a whole number"))
        })
        .transpose()
}
