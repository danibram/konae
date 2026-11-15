//! Execution policy for a Kernel process.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    #[default]
    Live,
    Test,
}

impl ExecutionMode {
    pub fn is_test(self) -> bool {
        self == Self::Test
    }

    pub fn parse(value: Option<&str>, bind: &str) -> anyhow::Result<Self> {
        let mode = match value.unwrap_or("live") {
            "live" => Self::Live,
            "test" => Self::Test,
            other => anyhow::bail!("KONAE_EXECUTION_MODE={other} is not valid. Use live or test."),
        };
        if mode.is_test() && !super::is_local_host(bind) {
            anyhow::bail!(
                "KONAE_EXECUTION_MODE=test requires a loopback KONAE_BIND. Set KONAE_BIND to 127.0.0.1 or ::1."
            );
        }
        Ok(mode)
    }
}

pub const TEST_CAPABILITY_VERSION: u8 = 1;

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"executionTest": TEST_CAPABILITY_VERSION})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_mode_defaults_to_live_and_rejects_unknown_or_reachable_test_mode() {
        assert_eq!(
            ExecutionMode::parse(None, "0.0.0.0").unwrap(),
            ExecutionMode::Live
        );
        assert_eq!(
            ExecutionMode::parse(Some("live"), "0.0.0.0").unwrap(),
            ExecutionMode::Live
        );
        assert_eq!(
            ExecutionMode::parse(Some("test"), "127.0.0.1").unwrap(),
            ExecutionMode::Test
        );
        assert_eq!(
            ExecutionMode::parse(Some("test"), "::1").unwrap(),
            ExecutionMode::Test
        );
        assert!(
            ExecutionMode::parse(Some("other"), "127.0.0.1")
                .unwrap_err()
                .to_string()
                .contains("Use live or test")
        );
        assert!(
            ExecutionMode::parse(Some("test"), "0.0.0.0")
                .unwrap_err()
                .to_string()
                .contains("requires a loopback")
        );
    }

    #[test]
    fn capabilities_publish_the_supported_test_policy_version() {
        assert_eq!(capabilities(), serde_json::json!({"executionTest": 1}));
    }
}
