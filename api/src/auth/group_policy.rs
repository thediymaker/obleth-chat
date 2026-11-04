//! Optional group policies consume only locally configured group IDs. They never
//! convert an external group name into an application role.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct GroupPolicy {
    defaults: Vec<Uuid>,
    claim: String,
    mapping: BTreeMap<String, Vec<Uuid>>,
    enabled: bool,
}
impl GroupPolicy {
    pub(super) fn from_env(oidc: bool) -> anyhow::Result<Self> {
        Self::parse(
            optional_env("OBLETH_CHAT_DEFAULT_GROUP_IDS").as_deref(),
            optional_env("OBLETH_CHAT_OIDC_GROUP_CLAIM").as_deref(),
            optional_env("OBLETH_CHAT_OIDC_GROUP_MAP").as_deref(),
            oidc,
        )
    }
    pub(super) fn parse(
        defaults: Option<&str>,
        claim: Option<&str>,
        mapping: Option<&str>,
        oidc: bool,
    ) -> anyhow::Result<Self> {
        if !oidc && (claim.is_some() || mapping.is_some()) {
            bail!("OIDC group mapping requires OIDC configuration");
        }
        let defaults = defaults
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Uuid::parse_str)
            .collect::<Result<BTreeSet<_>, _>>()
            .context("Default groups must be comma-separated UUIDs")?
            .into_iter()
            .collect::<Vec<_>>();
        let claim = claim.unwrap_or("groups").to_owned();
        if claim.is_empty() || claim.len() > 128 || claim.chars().any(char::is_control) {
            bail!("OIDC group claim must be 1–128 printable characters");
        }
        let parsed: BTreeMap<String, Vec<Uuid>> = serde_json::from_str(mapping.unwrap_or("{}"))
            .context("OIDC group map must map claim values to arrays of group UUIDs")?;
        if defaults.len() > 100
            || parsed.len() > 100
            || parsed.iter().any(|(k, v)| {
                k.is_empty() || k.len() > 256 || k.chars().any(char::is_control) || v.len() > 100
            })
        {
            bail!("Group policy exceeds its 100-group/rule limit");
        }
        Ok(Self {
            defaults,
            claim,
            mapping: parsed,
            enabled: mapping.is_some(),
        })
    }
    pub(super) async fn validate(&self, pool: &PgPool) -> anyhow::Result<()> {
        let ids = self
            .defaults
            .iter()
            .chain(self.mapping.values().flatten())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(());
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_groups WHERE id=ANY($1)")
            .bind(&ids)
            .fetch_one(pool)
            .await?;
        if count != ids.len() as i64 {
            bail!("Configured group policy references a group that does not exist");
        }
        Ok(())
    }
    pub(super) async fn initial(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
    ) -> Result<(), ApiError> {
        for group_id in &self.defaults {
            sqlx::query("INSERT INTO user_group_members(group_id,user_id) SELECT id,$2 FROM user_groups WHERE id=$1 ON CONFLICT DO NOTHING").bind(group_id).bind(user_id).execute(&mut **tx).await?;
        }
        Ok(())
    }
    /// The caller must first verify this exact ID token's signature, issuer,
    /// audience, expiry, nonce, azp and at_hash with openidconnect. CoreClient
    /// discards extension claims; reading its verified original payload here
    /// retains a configurable claim name without trusting an unverified JWT.
    pub(super) fn groups_from_verified_token(&self, token: &str) -> Result<Vec<Uuid>, ApiError> {
        if !self.enabled {
            return Ok(vec![]);
        }
        let payload = token
            .split('.')
            .nth(1)
            .filter(|s| s.len() <= 65536)
            .ok_or_else(ApiError::unauthorized)?;
        let raw = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| ApiError::unauthorized())?;
        let claims: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|_| ApiError::unauthorized())?;
        let external = match claims.get(&self.claim) {
            None | Some(serde_json::Value::Null) => vec![],
            Some(serde_json::Value::Array(items)) if items.len() <= 256 => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .filter(|s| s.len() <= 256)
                        .ok_or_else(ApiError::unauthorized)
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => {
                return Err(ApiError(
                    StatusCode::UNAUTHORIZED,
                    "OIDC groups claim must be an array of group names".into(),
                ))
            }
        };
        Ok(external
            .into_iter()
            .filter_map(|name| self.mapping.get(name))
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
    pub(super) async fn reconcile(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        groups: &[Uuid],
    ) -> Result<(), ApiError> {
        if !self.enabled {
            return Ok(());
        }
        // Manual membership always wins. Only rows originally inserted by this
        // policy are removed when the provider no longer grants their group.
        let removed=sqlx::query("DELETE FROM user_group_members WHERE user_id=$1 AND managed_oidc AND NOT(group_id=ANY($2))").bind(user_id).bind(groups).execute(&mut **tx).await?.rows_affected();
        let mut added = 0;
        for group_id in groups {
            added+=sqlx::query("INSERT INTO user_group_members(group_id,user_id,managed_oidc) SELECT id,$2,TRUE FROM user_groups WHERE id=$1 ON CONFLICT DO NOTHING").bind(group_id).bind(user_id).execute(&mut **tx).await?.rows_affected();
        }
        if added + removed > 0 {
            accounts::record_audit(
                tx,
                Some(user_id),
                "oidc_groups_reconciled",
                "user",
                Some(&user_id.to_string()),
                json!({"added":added,"removed":removed}),
            )
            .await?;
        }
        Ok(())
    }
}
