//! Chat-owned presentation and access policy over the Gateway's identity-scoped
//! catalog. Neither discovery nor Auto can grant access the gateway did not list.
use crate::{
    auth::Identity,
    error::ApiError,
    gateway::{DiscoveredModel, Gateway, ModelMetadata},
    store::{ReasoningEffort, MAX_SYSTEM_PROMPT_BYTES},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const PARAMETER_NAMES: [&str; 5] = [
    "systemPrompt",
    "temperature",
    "topP",
    "maxTokens",
    "reasoningEffort",
];
fn allowed_defaults() -> Vec<String> {
    PARAMETER_NAMES.iter().map(|s| (*s).into()).collect()
}
fn yes() -> bool {
    true
}
fn visibility_all() -> String {
    "all".into()
}

/// The highest output limit anyone may set; 0 means none.
pub const MAX_OUTPUT_TOKENS: i32 = 262_144;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
}
impl ParameterOverrides {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self
            .system_prompt
            .as_ref()
            .is_some_and(|v| v.len() > MAX_SYSTEM_PROMPT_BYTES || v.contains('\0'))
        {
            return Err(ApiError::bad(
                "System instructions must be at most 8 KiB and cannot contain NUL",
            ));
        }
        if self
            .temperature
            .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
        {
            return Err(ApiError::bad("Temperature must be between 0 and 2"));
        }
        if self
            .top_p
            .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err(ApiError::bad("Top P must be between 0 and 1"));
        }
        if self
            .max_tokens
            .is_some_and(|v| !(0..=MAX_OUTPUT_TOKENS).contains(&v))
        {
            return Err(ApiError::bad(
                "The output limit must be 0 (the model's own) or up to 262,144 tokens",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationParameters {
    pub system_prompt: String,
    pub temperature: f64,
    pub top_p: f64,
    /// An output limit someone chose; 0 leaves it to the model.
    pub max_tokens: i32,
    pub reasoning_effort: ReasoningEffort,
}
impl Default for GenerationParameters {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            temperature: 0.7,
            top_p: 1.0,
            max_tokens: 0,
            reasoning_effort: ReasoningEffort::Medium,
        }
    }
}
fn merge(base: &mut Value, patch: Value) {
    if let (Some(base), Some(patch)) = (base.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            base.insert(key.clone(), value.clone());
        }
    }
}
fn concrete(parameters: &ParameterOverrides) -> GenerationParameters {
    let mut value = json!(GenerationParameters::default());
    merge(&mut value, json!(parameters));
    serde_json::from_value(value).expect("validated typed parameters")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub order: i32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub published: bool,
    #[serde(default = "visibility_all")]
    pub visibility: String,
    #[serde(default)]
    pub group_ids: Vec<Uuid>,
    #[serde(default)]
    pub is_default: bool,
    /// Other models may hand tasks to this one (see agents).
    #[serde(default)]
    pub worker: bool,
    #[serde(default)]
    pub defaults: ParameterOverrides,
    #[serde(default = "allowed_defaults")]
    pub allowed_overrides: Vec<String>,
}
impl ProfileInput {
    fn validate(&mut self) -> Result<(), ApiError> {
        self.name = self.name.trim().into();
        self.description = self.description.trim().into();
        if self.name.is_empty()
            || self.name.chars().count() > 120
            || self.name.chars().any(char::is_control)
        {
            return Err(ApiError::bad(
                "Model name must contain 1–120 printable characters",
            ));
        }
        if self.description.chars().count() > 1000 || self.description.contains('\0') {
            return Err(ApiError::bad(
                "Description must be at most 1,000 characters",
            ));
        }
        if !(-10000..=10000).contains(&self.order) {
            return Err(ApiError::bad(
                "Model order must be between -10,000 and 10,000",
            ));
        }
        if self.tags.len() > 12
            || self.tags.iter().any(|v| {
                v.trim().is_empty() || v.chars().count() > 32 || v.chars().any(char::is_control)
            })
        {
            return Err(ApiError::bad("Use at most 12 tags of 1–32 characters"));
        }
        self.tags = self.tags.iter().map(|v| v.trim().to_owned()).collect();
        self.tags.sort();
        self.tags.dedup();
        if !matches!(self.visibility.as_str(), "all" | "admins" | "groups") {
            return Err(ApiError::bad("Visibility must be all, admins, or groups"));
        }
        if self.group_ids.len() > 100 {
            return Err(ApiError::bad("A model can reference at most 100 groups"));
        }
        self.group_ids.sort();
        self.group_ids.dedup();
        if self.visibility == "groups" && self.group_ids.is_empty() {
            return Err(ApiError::bad("Choose at least one group"));
        }
        if self.visibility != "groups" {
            self.group_ids.clear();
        }
        if self.is_default && (!self.enabled || !self.published) {
            return Err(ApiError::bad(
                "The default model must be enabled and published",
            ));
        }
        if self
            .allowed_overrides
            .iter()
            .any(|name| !PARAMETER_NAMES.contains(&name.as_str()))
        {
            return Err(ApiError::bad("Unknown override parameter"));
        }
        self.allowed_overrides.sort();
        self.allowed_overrides.dedup();
        self.defaults.validate()
    }
}
#[derive(Clone, FromRow)]
struct ProfileRow {
    model_id: String,
    name: String,
    description: String,
    tags: Value,
    sort_order: i32,
    enabled: bool,
    published: bool,
    visibility: String,
    is_default: bool,
    worker: bool,
    defaults: Value,
    allowed_overrides: Value,
    group_ids: Vec<Uuid>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    pub name: String,
    pub detail: String,
    pub description: String,
    pub tags: Vec<String>,
    pub order: i32,
    pub enabled: bool,
    pub published: bool,
    pub visibility: String,
    pub group_ids: Vec<Uuid>,
    pub is_default: bool,
    pub worker: bool,
    pub defaults: GenerationParameters,
    pub allowed_overrides: Vec<String>,
    pub locked_parameters: Vec<String>,
    pub available: bool,
    #[serde(skip)]
    discovered: bool,
    pub metadata: ModelMetadata,
}
impl ModelView {
    fn from_parts(model: Option<DiscoveredModel>, profile: Option<ProfileRow>) -> Self {
        let discovered = model.is_some();
        let available = model
            .as_ref()
            .is_some_and(|model| model.metadata.healthy != Some(false));
        let id = model
            .as_ref()
            .map(|m| m.id.clone())
            .or_else(|| profile.as_ref().map(|p| p.model_id.clone()))
            .unwrap();
        let name = profile
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| id.clone());
        let description = profile
            .as_ref()
            .map(|p| p.description.clone())
            .unwrap_or_default();
        let defaults = profile
            .as_ref()
            .and_then(|p| serde_json::from_value::<ParameterOverrides>(p.defaults.clone()).ok())
            .unwrap_or_default();
        let allowed = profile
            .as_ref()
            .and_then(|p| serde_json::from_value::<Vec<String>>(p.allowed_overrides.clone()).ok())
            .unwrap_or_else(allowed_defaults);
        Self {
            id,
            name,
            detail: description.clone(),
            description,
            tags: profile
                .as_ref()
                .and_then(|p| serde_json::from_value(p.tags.clone()).ok())
                .unwrap_or_else(|| model.as_ref().map(|m| m.tags.clone()).unwrap_or_default()),
            order: profile.as_ref().map(|p| p.sort_order).unwrap_or(0),
            enabled: profile.as_ref().is_none_or(|p| p.enabled),
            published: profile.as_ref().is_none_or(|p| p.published),
            visibility: profile
                .as_ref()
                .map(|p| p.visibility.clone())
                .unwrap_or_else(visibility_all),
            group_ids: profile
                .as_ref()
                .map(|p| p.group_ids.clone())
                .unwrap_or_default(),
            is_default: profile.as_ref().is_some_and(|p| p.is_default),
            worker: profile.as_ref().is_some_and(|p| p.worker),
            defaults: concrete(&defaults),
            locked_parameters: PARAMETER_NAMES
                .iter()
                .filter(|name| !allowed.iter().any(|v| v == **name))
                .map(|s| (*s).into())
                .collect(),
            allowed_overrides: allowed,
            available,
            discovered,
            metadata: model.map(|m| m.metadata).unwrap_or_default(),
        }
    }
    fn accessible(&self, user: &Identity, groups: &HashSet<Uuid>) -> bool {
        self.discovered
            && self.enabled
            && self.published
            && (user.is_admin()
                || self.visibility == "all"
                || (self.visibility == "groups"
                    && self.group_ids.iter().any(|id| groups.contains(id))))
    }
}

async fn profiles(pool: &PgPool) -> Result<Vec<ProfileRow>, ApiError> {
    Ok(sqlx::query_as::<_, ProfileRow>("SELECT p.*, ARRAY(SELECT group_id FROM model_profile_groups g WHERE g.model_id=p.model_id ORDER BY group_id) AS group_ids FROM model_profiles p").fetch_all(pool).await?)
}
pub async fn effective_catalog(
    pool: &PgPool,
    gateway: &Gateway,
    user: &Identity,
    include_unpublished: bool,
) -> Result<Vec<ModelView>, ApiError> {
    let discovered = gateway.discover_models(user).await?;
    // Re-read authorization after network discovery: a role revocation or disable
    // during a slow gateway request must not retain an earlier administrator role.
    let (saved, group_ids, current_user) = tokio::try_join!(
        profiles(pool),
        async {
            Ok::<_, ApiError>(
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT group_id FROM user_group_members WHERE user_id=$1",
                )
                .bind(user.id)
                .fetch_all(pool)
                .await?,
            )
        },
        async {
            sqlx::query_as::<_, Identity>(
                "SELECT id,username,display_name,role FROM users WHERE id=$1 AND enabled",
            )
            .bind(user.id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(ApiError::unauthorized)
        }
    )?;
    let groups: HashSet<_> = group_ids.into_iter().collect();
    let mut saved: HashMap<_, _> = saved.into_iter().map(|p| (p.model_id.clone(), p)).collect();
    let mut result: Vec<_> = discovered
        .into_iter()
        .map(|m| {
            let profile = saved.remove(&m.id);
            ModelView::from_parts(Some(m), profile)
        })
        .collect();
    if include_unpublished && current_user.is_admin() {
        result.extend(
            saved
                .into_values()
                .map(|p| ModelView::from_parts(None, Some(p))),
        );
    } else {
        result.retain(|m| m.accessible(&current_user, &groups));
    }
    result.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(result)
}

/// The worker models this person may use, by id and name, in catalog order. From
/// the saved policy alone: whether the gateway still serves one is checked when a
/// worker starts.
pub async fn worker_models(pool: &PgPool, user: &Identity) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT p.model_id, p.name FROM model_profiles p WHERE p.worker AND p.enabled AND p.published
         AND ($2 OR p.visibility='all' OR (p.visibility='groups' AND EXISTS(SELECT 1 FROM model_profile_groups g
             JOIN user_group_members m ON m.group_id=g.group_id WHERE g.model_id=p.model_id AND m.user_id=$1)))
         ORDER BY p.sort_order, LOWER(p.name), p.model_id LIMIT 12",
    )
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// The person's saved defaults, for a chat started from outside the browser.
pub async fn preferences(pool: &PgPool, user_id: Uuid) -> Preferences {
    raw_preferences(pool, user_id)
        .await
        .ok()
        .and_then(|raw| preferences_value(raw).ok())
        .unwrap_or_default()
}
/// The models a person may pick, Auto first, as the model chooser lists them.
pub async fn choices(
    pool: &PgPool,
    gateway: &Gateway,
    user: &Identity,
) -> Result<Vec<ModelView>, ApiError> {
    let models = effective_catalog(pool, gateway, user, false).await?;
    let mut result: Vec<ModelView> = auto_model(&models).into_iter().collect();
    result.extend(models);
    Ok(result)
}
async fn raw_preferences(pool: &PgPool, user_id: Uuid) -> Result<Value, ApiError> {
    Ok(
        sqlx::query_scalar::<_, Value>("SELECT preferences FROM user_preferences WHERE user_id=$1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_else(|| json!({})),
    )
}
/// Auto is the gateway's router: Chat sends `auto` and the gateway picks a model that
/// can handle the request (tools, images, context) for each message. Chat does not
/// know which one until the reply streams, so Auto carries no per-model policy.
fn auto_model(models: &[ModelView]) -> Option<ModelView> {
    models.iter().any(|m| m.available).then(|| ModelView {
        id: "auto".into(),
        name: "Auto".into(),
        detail: "The gateway picks a model for each message".into(),
        description: "The gateway picks a model for each message, based on what it needs (tools, images, a long context) and how hard it is.".into(),
        tags: vec![],
        order: i32::MIN,
        enabled: true,
        published: true,
        visibility: "all".into(),
        group_ids: vec![],
        is_default: false,
        worker: false,
        defaults: GenerationParameters::default(),
        allowed_overrides: allowed_defaults(),
        locked_parameters: vec![],
        available: true,
        discovered: true,
        metadata: ModelMetadata {
            mode: "chat".into(),
            // The gateway only routes requests to models that support what they use.
            supports_vision: Some(true),
            supports_function_calling: Some(true),
            healthy: Some(true),
            ..ModelMetadata::default()
        },
    })
}
pub struct ResolvedRequest {
    pub model: String,
    pub requested_model: String,
    pub parameters: GenerationParameters,
    pub capabilities: ModelMetadata,
    /// With Auto, the gateway models this person may use: a tool round stays on the
    /// model that answered only when it is one of these.
    pub choices: Vec<String>,
}
/// The gateway model behind a name an upstream reported. Upstreams often report
/// their own name for a model, such as `qwen35-122b-a10b-fp8` for the gateway's
/// `qwen35-122b-a10b`, and the gateway accepts only its own names.
pub fn gateway_name<'a>(reported: &str, choices: &'a [String]) -> Option<&'a str> {
    let reported = reported.rsplit('/').next().unwrap_or(reported);
    choices
        .iter()
        .filter(|name| {
            reported == name.as_str()
                || reported
                    .strip_prefix(name.as_str())
                    .is_some_and(|rest| rest.starts_with(['-', '_', '.', ':', '@']))
        })
        .max_by_key(|name| name.len())
        .map(String::as_str)
}
pub async fn resolve_request(
    pool: &PgPool,
    gateway: &Gateway,
    user: &Identity,
    requested_model: &str,
    requested: ParameterOverrides,
) -> Result<ResolvedRequest, ApiError> {
    requested.validate()?;
    let (models, preferences) = tokio::try_join!(
        effective_catalog(pool, gateway, user, false),
        raw_preferences(pool, user.id)
    )?;
    resolve_from_catalog(&models, &preferences, requested_model, requested)
}
fn resolve_from_catalog(
    models: &[ModelView],
    preferences: &Value,
    requested_model: &str,
    requested: ParameterOverrides,
) -> Result<ResolvedRequest, ApiError> {
    let auto = (requested_model == "auto")
        .then(|| auto_model(models))
        .flatten();
    let model = auto
        .as_ref()
        .or_else(|| models.iter().find(|m| m.id == requested_model))
        .ok_or_else(|| {
            ApiError(
                StatusCode::FORBIDDEN,
                "This model is not available to your account".into(),
            )
        })?;
    if !model.available {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "This model is currently unavailable. Choose another model or try again later.".into(),
        ));
    }
    let mut parameters = json!(model.defaults);
    let explicit = json!(requested);
    for key in PARAMETER_NAMES {
        if model.allowed_overrides.iter().any(|allowed| allowed == key) {
            if let Some(value) = preferences.get(key) {
                parameters[key] = value.clone();
            }
            if let Some(value) = explicit.get(key) {
                parameters[key] = value.clone();
            }
        } else if explicit
            .get(key)
            .is_some_and(|value| value != &parameters[key])
        {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                format!("Your administrator has locked {key} for this model"),
            ));
        }
    }
    let mut parameters: GenerationParameters =
        serde_json::from_value(parameters).map_err(|_| ApiError::internal())?;
    // A saved preference must not break a model with a smaller window: clamp to it.
    if let Some(limit) = model.metadata.context_window {
        if parameters.max_tokens as u64 > limit {
            parameters.max_tokens = i32::try_from(limit).unwrap_or(i32::MAX);
        }
    }
    Ok(ResolvedRequest {
        model: model.id.clone(),
        requested_model: requested_model.into(),
        parameters,
        capabilities: model.metadata.clone(),
        choices: if auto.is_some() {
            models
                .iter()
                .filter(|m| m.available)
                .map(|m| m.id.clone())
                .collect()
        } else {
            vec![]
        },
    })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/models", get(catalog))
        .route("/api/admin/models", get(admin_catalog))
        .route("/api/admin/models/{*id}", put(save_profile))
        .route(
            "/api/preferences",
            get(get_preferences).patch(save_preferences),
        )
        .route("/api/presets", get(list_presets).post(create_preset))
        .route(
            "/api/presets/{id}",
            axum::routing::patch(update_preset).delete(delete_preset),
        )
        .with_state(state)
}
fn gateway(state: &AppState) -> Result<&Gateway, ApiError> {
    state.gateway.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Gateway integration is not configured".into(),
        )
    })
}
async fn catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, false).await?;
    let result = choices(&state.store.pool, gateway(&state)?, &user).await?;
    Ok(Json(json!({"models":result})))
}
async fn admin_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require_admin(&headers, false).await?;
    Ok(Json(
        json!({"models":effective_catalog(&state.store.pool,gateway(&state)?,&user,true).await?}),
    ))
}
async fn admin_transaction(
    pool: &PgPool,
    user: Uuid,
) -> Result<Transaction<'_, Postgres>, ApiError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(7346281951102)")
        .execute(&mut *tx)
        .await?;
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND enabled AND role='admin')",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    if !allowed {
        return Err(ApiError::forbidden());
    }
    Ok(tx)
}
async fn save_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut body): Json<ProfileInput>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require_admin(&headers, true).await?;
    if !crate::gateway::valid_model(&id) || id == "auto" {
        return Err(ApiError::bad("Choose an explicit gateway model"));
    }
    body.validate()?;
    let models = effective_catalog(&state.store.pool, gateway(&state)?, &user, true).await?;
    let mut saved_model = models
        .into_iter()
        .find(|model| model.id == id)
        .ok_or_else(ApiError::not_found)?;
    let mut tx = admin_transaction(&state.store.pool, user.id).await?;
    if !body.group_ids.is_empty() {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_groups WHERE id=ANY($1)")
            .bind(&body.group_ids)
            .fetch_one(&mut *tx)
            .await?;
        if count != body.group_ids.len() as i64 {
            return Err(ApiError::bad("One or more groups no longer exist"));
        }
    }
    if body.is_default {
        sqlx::query(
            "UPDATE model_profiles SET is_default=FALSE, updated_at=NOW() WHERE is_default",
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("INSERT INTO model_profiles (model_id,name,description,tags,sort_order,enabled,published,visibility,is_default,defaults,allowed_overrides,worker) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(model_id) DO UPDATE SET name=EXCLUDED.name,description=EXCLUDED.description,tags=EXCLUDED.tags,sort_order=EXCLUDED.sort_order,enabled=EXCLUDED.enabled,published=EXCLUDED.published,visibility=EXCLUDED.visibility,is_default=EXCLUDED.is_default,defaults=EXCLUDED.defaults,allowed_overrides=EXCLUDED.allowed_overrides,worker=EXCLUDED.worker,updated_at=NOW()")
        .bind(&id).bind(&body.name).bind(&body.description).bind(json!(body.tags)).bind(body.order).bind(body.enabled).bind(body.published).bind(&body.visibility).bind(body.is_default).bind(json!(body.defaults)).bind(json!(body.allowed_overrides)).bind(body.worker).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM model_profile_groups WHERE model_id=$1")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    if !body.group_ids.is_empty() {
        sqlx::query(
            "INSERT INTO model_profile_groups(model_id,group_id) SELECT $1, UNNEST($2::uuid[])",
        )
        .bind(&id)
        .bind(&body.group_ids)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,target_id,metadata) VALUES($1,$2,'model.update','model',$3,$4)").bind(Uuid::new_v4()).bind(user.id).bind(&id).bind(json!({"enabled":body.enabled,"published":body.published,"visibility":body.visibility,"isDefault":body.is_default,"worker":body.worker,"allowedOverrides":body.allowed_overrides})).execute(&mut *tx).await?;
    tx.commit().await?;
    // A successful local policy write must not appear to fail because a second
    // network-discovery request happened to fail after the transaction committed.
    saved_model.name = body.name;
    saved_model.description = body.description;
    saved_model.detail = saved_model.description.clone();
    saved_model.tags = body.tags;
    saved_model.order = body.order;
    saved_model.enabled = body.enabled;
    saved_model.published = body.published;
    saved_model.visibility = body.visibility;
    saved_model.group_ids = body.group_ids;
    saved_model.is_default = body.is_default;
    saved_model.worker = body.worker;
    saved_model.defaults = concrete(&body.defaults);
    saved_model.locked_parameters = PARAMETER_NAMES
        .iter()
        .filter(|name| !body.allowed_overrides.iter().any(|v| v == **name))
        .map(|s| (*s).into())
        .collect();
    saved_model.allowed_overrides = body.allowed_overrides;
    Ok(Json(json!({"model":saved_model})))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub default_model: String,
    pub reasoning_effort: ReasoningEffort,
    pub system_prompt: String,
    pub temperature: f64,
    pub top_p: f64,
    pub max_tokens: i32,
    pub density: String,
    pub focus: bool,
    pub context: bool,
    pub favorites: Vec<String>,
    /// How the workspace agent works; None uses Chat's default instructions.
    pub agent_instructions: Option<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            default_model: "auto".into(),
            reasoning_effort: ReasoningEffort::Medium,
            system_prompt: String::new(),
            temperature: 0.7,
            top_p: 1.0,
            max_tokens: 0,
            density: "comfortable".into(),
            focus: false,
            context: true,
            favorites: vec![],
            agent_instructions: None,
        }
    }
}

/// The person's own workspace agent instructions, when they replaced the default.
pub async fn agent_instructions(pool: &PgPool, user_id: Uuid) -> Option<String> {
    let raw = raw_preferences(pool, user_id).await.ok()?;
    raw.get("agentInstructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
fn preferences_value(raw: Value) -> Result<Preferences, ApiError> {
    let mut value = json!(Preferences::default());
    merge(&mut value, raw);
    serde_json::from_value(value).map_err(|_| ApiError::bad("Invalid personal preferences"))
}
async fn get_preferences(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, false).await?;
    let overrides = raw_preferences(&state.store.pool, user.id).await?;
    Ok(Json(
        json!({"preferences":preferences_value(overrides.clone())?,"overrides":overrides,"defaultAgentInstructions":crate::sandbox::tools::DEFAULT_INSTRUCTIONS}),
    ))
}
async fn owner_transaction(
    pool: &PgPool,
    user: Uuid,
) -> Result<Transaction<'_, Postgres>, ApiError> {
    let mut tx = pool.begin().await?;
    let exists =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id=$1 AND enabled FOR UPDATE")
            .bind(user)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(ApiError::unauthorized());
    }
    Ok(tx)
}
fn validate_preferences(preferences: &Preferences) -> Result<(), ApiError> {
    if preferences
        .agent_instructions
        .as_ref()
        .is_some_and(|text| text.len() > 16 * 1024 || text.contains('\0'))
    {
        return Err(ApiError::bad(
            "Agent instructions must be at most 16 KiB and cannot contain NUL",
        ));
    }
    ParameterOverrides {
        system_prompt: Some(preferences.system_prompt.clone()),
        temperature: Some(preferences.temperature),
        top_p: Some(preferences.top_p),
        max_tokens: Some(preferences.max_tokens),
        reasoning_effort: Some(preferences.reasoning_effort),
    }
    .validate()?;
    if !crate::gateway::valid_model(&preferences.default_model) {
        return Err(ApiError::bad("Choose a valid default model"));
    }
    if !matches!(preferences.density.as_str(), "comfortable" | "compact") {
        return Err(ApiError::bad("Density must be comfortable or compact"));
    }
    if preferences.favorites.len() > 100
        || preferences
            .favorites
            .iter()
            .any(|id| !crate::gateway::valid_model(id) || id == "auto")
    {
        return Err(ApiError::bad("Choose at most 100 explicit favorite models"));
    }
    Ok(())
}
async fn save_preferences(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    if !body.is_object() {
        return Err(ApiError::bad("Preferences must be an object"));
    }
    // Validate the submitted document before taking a lock or making network calls.
    let submitted = preferences_value(body.clone())?;
    validate_preferences(&submitted)?;
    if body.get("defaultModel").is_some_and(|v| v != "auto")
        || body
            .get("favorites")
            .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
    {
        let catalog = effective_catalog(&state.store.pool, gateway(&state)?, &user, false).await?;
        if body.get("defaultModel").is_some()
            && submitted.default_model != "auto"
            && !catalog.iter().any(|m| m.id == submitted.default_model)
        {
            return Err(ApiError::forbidden());
        }
        if body.get("favorites").is_some()
            && submitted
                .favorites
                .iter()
                .any(|id| !catalog.iter().any(|m| &m.id == id))
        {
            return Err(ApiError::forbidden());
        }
    }
    let mut tx = owner_transaction(&state.store.pool, user.id).await?;
    let mut raw =
        sqlx::query_scalar::<_, Value>("SELECT preferences FROM user_preferences WHERE user_id=$1")
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or_else(|| json!({}));
    merge(&mut raw, body);
    let mut preferences = preferences_value(raw.clone())?;
    validate_preferences(&preferences)?;
    preferences.favorites.sort();
    preferences.favorites.dedup();
    if raw.get("favorites").is_some() {
        raw["favorites"] = json!(preferences.favorites);
    }
    sqlx::query("INSERT INTO user_preferences(user_id,preferences) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET preferences=EXCLUDED.preferences,updated_at=NOW()").bind(user.id).bind(&raw).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"preferences":preferences,"overrides":raw,"defaultAgentInstructions":crate::sandbox::tools::DEFAULT_INSTRUCTIONS}),
    ))
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Preset {
    id: Uuid,
    name: String,
    model: String,
    parameters: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresetInput {
    name: String,
    model: String,
    #[serde(default)]
    parameters: ParameterOverrides,
}
async fn list_presets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, false).await?;
    let presets=sqlx::query_as::<_,Preset>("SELECT id,name,model,parameters FROM user_presets WHERE user_id=$1 ORDER BY updated_at DESC,id").bind(user.id).fetch_all(&state.store.pool).await?;
    Ok(Json(json!({"presets":presets})))
}
async fn create_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PresetInput>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = state.auth.require(&headers, true).await?;
    let preset = save_preset(&state, &user, None, body).await?;
    Ok((StatusCode::CREATED, Json(json!({"preset":preset}))))
}
async fn update_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PresetInput>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    Ok(Json(
        json!({"preset":save_preset(&state,&user,Some(id),body).await?}),
    ))
}
async fn save_preset(
    state: &AppState,
    user: &Identity,
    id: Option<Uuid>,
    mut body: PresetInput,
) -> Result<Preset, ApiError> {
    body.name = body.name.trim().into();
    if body.name.is_empty()
        || body.name.chars().count() > 80
        || body.name.chars().any(char::is_control)
    {
        return Err(ApiError::bad(
            "Preset name must contain 1–80 printable characters",
        ));
    }
    // Store only a usable model/parameter combination. Resolve again at send time,
    // because later model restrictions must apply to previously saved presets.
    resolve_request(
        &state.store.pool,
        gateway(state)?,
        user,
        &body.model,
        body.parameters.clone(),
    )
    .await?;
    let mut tx = owner_transaction(&state.store.pool, user.id).await?;
    let preset = if let Some(id) = id {
        sqlx::query_as::<_,Preset>("UPDATE user_presets SET name=$3,model=$4,parameters=$5,updated_at=NOW() WHERE id=$1 AND user_id=$2 RETURNING id,name,model,parameters").bind(id).bind(user.id).bind(body.name).bind(body.model).bind(json!(body.parameters)).fetch_optional(&mut *tx).await
    } else {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_presets WHERE user_id=$1")
            .bind(user.id)
            .fetch_one(&mut *tx)
            .await?;
        if count >= 50 {
            return Err(ApiError::bad("You can save at most 50 presets"));
        }
        sqlx::query_as::<_,Preset>("INSERT INTO user_presets(id,user_id,name,model,parameters) VALUES($1,$2,$3,$4,$5) RETURNING id,name,model,parameters").bind(Uuid::new_v4()).bind(user.id).bind(body.name).bind(body.model).bind(json!(body.parameters)).fetch_optional(&mut *tx).await
    };
    let preset = match preset {
        Ok(Some(preset)) => preset,
        Ok(None) => return Err(ApiError::not_found()),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "A preset with this name already exists".into(),
            ))
        }
        Err(error) => return Err(error.into()),
    };
    tx.commit().await?;
    Ok(preset)
}
async fn delete_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    let mut tx = owner_transaction(&state.store.pool, user.id).await?;
    if sqlx::query("DELETE FROM user_presets WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 0
    {
        return Err(ApiError::not_found());
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::gateway_name;

    #[test]
    fn maps_upstream_names_to_gateway_models() {
        let choices: Vec<String> = [
            "qwen35-122b-a10b",
            "qwen35-122b",
            "glm-5-3",
            "glm-5-3-flash",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            gateway_name("qwen35-122b-a10b-fp8", &choices),
            Some("qwen35-122b-a10b")
        );
        assert_eq!(
            gateway_name("glm-5-3-flash", &choices),
            Some("glm-5-3-flash")
        );
        assert_eq!(gateway_name("zai/glm-5-3", &choices), Some("glm-5-3"));
        assert_eq!(gateway_name("glm-5-30", &choices), None);
        assert_eq!(gateway_name("llama-4", &choices), None);
    }
}
