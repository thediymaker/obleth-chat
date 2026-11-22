//! The audit log: one writer and one table of the actions it holds.
//!
//! Every event the Activity page shows is written through [`record`], and every
//! action name it stores is in [`Action`]'s table with the kind the page filters
//! by. Stored names are fixed: old rows keep them, so a renamed action needs its
//! old name kept here as well. New actions take a dotted, singular name,
//! `<feature>.<verb>`.

use serde_json::Value;
use sqlx::PgExecutor;
use uuid::Uuid;

/// What the Activity page's "Kind of activity" filter offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Accounts,
    SignIn,
    Groups,
    Settings,
    Connections,
    Mcp,
    Apps,
    Cleanup,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Accounts,
        Kind::SignIn,
        Kind::Groups,
        Kind::Settings,
        Kind::Connections,
        Kind::Mcp,
        Kind::Apps,
        Kind::Cleanup,
    ];

    /// The name the API and the UI use for the kind.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Accounts => "accounts",
            Kind::SignIn => "sign_in",
            Kind::Groups => "groups",
            Kind::Settings => "settings",
            Kind::Connections => "connections",
            Kind::Mcp => "mcp",
            Kind::Apps => "apps",
            Kind::Cleanup => "cleanup",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// The stored action names a filter on this kind matches.
    pub fn actions(self) -> Vec<&'static str> {
        Action::ALL
            .iter()
            .filter(|action| action.kind == self)
            .map(|action| action.name)
            .collect()
    }
}

/// One thing the audit log records, by its stored name and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub name: &'static str,
    pub kind: Kind,
}

macro_rules! actions {
    ($($ident:ident => $name:literal, $kind:ident;)*) => {
        impl Action {
            $(pub const $ident: Action = Action { name: $name, kind: Kind::$kind };)*
            /// Every action, in the order of this table.
            pub const ALL: &'static [Action] = &[$(Action::$ident),*];
        }
    };
}

actions! {
    USER_CREATED => "user_created", Accounts;
    USER_INVITED => "user_invited", Accounts;
    USER_UPDATED => "user_updated", Accounts;
    PROFILE_UPDATED => "profile_updated", Accounts;
    PASSWORD_CHANGED => "password_changed", Accounts;
    PASSWORD_RESET => "password_reset", Accounts;
    OIDC_LINKED => "oidc_linked", Accounts;
    ACCESS_LINK_CREATED => "access_link_created", Accounts;
    ACCESS_LINK_REDEEMED => "access_link_redeemed", Accounts;
    SESSION_REVOKED => "session_revoked", SignIn;
    OTHER_SESSIONS_REVOKED => "other_sessions_revoked", SignIn;
    USER_SESSIONS_REVOKED => "user_sessions_revoked", SignIn;
    SIGN_IN_ENDED => "sign_in_ended", SignIn;
    API_TOKEN_CREATED => "api_token_created", SignIn;
    API_TOKEN_REVOKED => "api_token_revoked", SignIn;
    GROUP_CREATED => "group_created", Groups;
    GROUP_UPDATED => "group_updated", Groups;
    GROUP_DELETED => "group_deleted", Groups;
    GROUP_MEMBER_ADDED => "group_member_added", Groups;
    GROUP_MEMBER_REMOVED => "group_member_removed", Groups;
    OIDC_GROUPS_RECONCILED => "oidc_groups_reconciled", Groups;
    MODEL_UPDATE => "model.update", Settings;
    MODEL_CERTIFY => "model.certify", Settings;
    MODELS_SHORT_LIST => "models.short_list", Settings;
    LIMITS_UPDATE => "limits.update", Settings;
    DECISIONS_UPDATE => "decisions.update", Settings;
    BANNER_UPDATE => "banner.update", Settings;
    APPEARANCE_UPDATE => "appearance.update", Settings;
    KNOWLEDGE_EMBEDDING_UPDATE => "knowledge.embedding.update", Settings;
    RETENTION_UPDATE => "retention.update", Settings;
    INSIGHTS_UPDATE => "insights.update", Settings;
    INSIGHTS_REPORT => "insights.report", Settings;
    MEMORY_UPDATE => "memory.update", Settings;
    NOTEBOOK_UPDATE => "notebook.update", Settings;
    FEEDBACK_BUTTON => "feedback.button", Settings;
    WORKSPACES_STOP => "workspaces.stop", Settings;
    CONNECTION_CREATE => "connection.create", Connections;
    CONNECTION_UPDATE => "connection.update", Connections;
    CONNECTION_DELETE => "connection.delete", Connections;
    MCP_CHAT_STARTED => "mcp_chat_started", Mcp;
    MCP_MESSAGE_SENT => "mcp_message_sent", Mcp;
    MCP_CHAT_STOPPED => "mcp_chat_stopped", Mcp;
    MCP_CHAT_DELETED => "mcp_chat_deleted", Mcp;
    APP_PUBLISHED => "app.published", Apps;
    APP_UNPUBLISHED => "app.unpublished", Apps;
    APP_DELETED => "app.deleted", Apps;
    APP_KEY_CREATED => "app.key_created", Apps;
    APP_KEY_REVOKED => "app.key_revoked", Apps;
    RETENTION_CLEANUP => "retention.cleanup", Cleanup;
}

impl Action {
    /// The kind of a stored action name; `None` for one this build does not know.
    pub fn kind_of(name: &str) -> Option<Kind> {
        Action::ALL
            .iter()
            .find(|action| action.name == name)
            .map(|action| action.kind)
    }
}

/// What an event is about. The Activity page joins the id to a name by the
/// target kind, so the kinds here are the ones its query knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: &'static str,
    pub id: Option<String>,
}

impl Target {
    /// The whole site: a setting with no row of its own.
    pub const fn site() -> Target {
        Target {
            kind: "site",
            id: None,
        }
    }
    fn of(kind: &'static str, id: impl ToString) -> Target {
        Target {
            kind,
            id: Some(id.to_string()),
        }
    }
    pub fn user(id: Uuid) -> Target {
        Target::of("user", id)
    }
    pub fn group(id: Uuid) -> Target {
        Target::of("group", id)
    }
    pub fn session(id: Uuid) -> Target {
        Target::of("session", id)
    }
    pub fn api_token(id: Uuid) -> Target {
        Target::of("api_token", id)
    }
    pub fn conversation(id: Uuid) -> Target {
        Target::of("conversation", id)
    }
    pub fn connection(id: Uuid) -> Target {
        Target::of("connection", id)
    }
    pub fn model(id: &str) -> Target {
        Target::of("model", id)
    }
    pub fn app(id: &str) -> Target {
        Target::of("app", id)
    }
}

/// Write one event. `db` is a pool or an open transaction (`&mut *tx`), so a
/// handler that already holds a transaction records inside it and one that
/// does not needs none.
pub async fn record<'e>(
    db: impl PgExecutor<'e>,
    actor: Option<Uuid>,
    action: Action,
    target: Target,
    metadata: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,target_id,metadata) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(Uuid::new_v4())
        .bind(actor)
        .bind(action.name)
        .bind(target.kind)
        .bind(target.id)
        .bind(metadata)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_action_has_one_name_and_every_kind_filter_covers_its_actions() {
        let names: HashSet<&str> = Action::ALL.iter().map(|a| a.name).collect();
        assert_eq!(
            names.len(),
            Action::ALL.len(),
            "two actions share a stored name"
        );
        let covered: usize = Kind::ALL.iter().map(|kind| kind.actions().len()).sum();
        assert_eq!(covered, Action::ALL.len());
        for action in Action::ALL {
            assert_eq!(
                Action::kind_of(action.name),
                Some(action.kind),
                "{}",
                action.name
            );
            assert_eq!(Kind::from_name(action.kind.name()), Some(action.kind));
        }
        assert_eq!(Action::kind_of("something_new"), None);
    }

    /// The Activity page reads every stored name as a sentence and offers every
    /// kind as a filter; its table lives in the UI, so this reads that file.
    #[test]
    fn the_activity_page_knows_every_action_and_kind() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/components/admin/audit.ts");
        let ui = std::fs::read_to_string(path).expect("the UI's audit sentence table");
        for action in Action::ALL {
            let key = if action.name.contains('.') {
                format!("\"{}\": ()", action.name)
            } else {
                format!("{}: ()", action.name)
            };
            assert!(
                ui.contains(&key),
                "no Activity sentence for {}",
                action.name
            );
        }
        for kind in Kind::ALL {
            assert!(
                ui.contains(&format!("[\"{}\", ", kind.name())),
                "no Activity filter for {}",
                kind.name()
            );
        }
    }
}
