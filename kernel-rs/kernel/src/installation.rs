
use std::path::Path;

#[derive(Clone, Debug)]
pub struct RootDatabase {
    pub file: &'static str,
    /// The tables a restore empties, so no session, credential or open link
    /// revoked after the copy comes back.
    pub sign_out: &'static [&'static str],
}

/// An account database at the data root, outside every app folder.
#[derive(Clone, Debug)]
pub struct AccountDatabase {
    /// The scope its challenges carry in the email queue.
    pub scope: &'static str,
    /// The kind its suspensions take in the revocation record.
    pub kind: &'static str,
    pub file: &'static str,
}

#[derive(Clone, Debug)]
pub struct Installation {
    pub databases: Vec<RootDatabase>,
    pub accounts: Vec<AccountDatabase>,
    pub check_catalog: fn(&Path) -> anyhow::Result<()>,
    pub check_cut: fn(&Path) -> anyhow::Result<()>,
    /// Who answers for an app's daily email allowance, by project ID. `None`
    /// counts the app under the installation.
    pub mail_group: fn(&Path, &str) -> Option<String>,
}

impl Installation {
    /// The Kernel's own stores: the email queue, the call outbox and the
    /// unfinished transfers.
    pub fn kernel() -> Self {
        Self {
            databases: vec![
                RootDatabase { file: crate::mail::MAIL_DATABASE, sign_out: &[] },
                RootDatabase { file: crate::calls::CALLS_DATABASE, sign_out: &[] },
                RootDatabase { file: crate::transfers::TRANSFERS_DATABASE, sign_out: &["transfers"] },
            ],
            accounts: Vec::new(),
            check_catalog: |_| Ok(()),
            check_cut: |_| Ok(()),
            mail_group: |_, _| None,
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn for_test() -> Self {
        let mut installation = Self::kernel();
        installation.databases.push(RootDatabase { file: "accounts.sqlite", sign_out: &["app_sessions", "account_challenges"] });
        installation.accounts.push(AccountDatabase { scope: "installation", kind: "installation-account", file: "accounts.sqlite" });
        installation.mail_group = |_, _| Some("group-1".into());
        installation
    }

    pub fn holds(&self, file: &str) -> bool {
        self.databases.iter().any(|database| database.file == file)
    }

    /// The account database whose record entries name `kind`.
    pub fn account_for_kind(&self, kind: &str) -> Option<&AccountDatabase> {
        self.accounts.iter().find(|account| account.kind == kind)
    }
}
