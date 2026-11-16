
use super::*;

#[derive(Clone)]
pub(crate) struct Konae {
    pub kernel: AppState,
    /// `KONAE_SETUP_KEY`: signs up the first owner and keys the owner forms.
    pub setup_key: String,
    /// `KONAE_MCP_ORIGIN` or its derived value; `None` keeps the request host.
    pub mcp_origin: Option<String>,
    /// How a new owner account proves its email.
    pub owner_verification: auth::Verification,
}

impl std::ops::Deref for Konae {
    type Target = AppState;

    fn deref(&self) -> &AppState {
        &self.kernel
    }
}

#[cfg(test)]
impl Konae {
    pub(crate) fn for_test(kernel: AppState) -> Self {
        let owner_verification = kernel.accounts.verification;
        Self { kernel, setup_key: "test-admin".into(), mcp_origin: None, owner_verification }
    }
}
