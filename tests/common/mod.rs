//! Shorthands the tests use for chats, built on Store's own methods.
// Each test binary uses only some of them.
#![allow(dead_code, async_fn_in_trait)]

use obleth_chat::{
    error::ApiError,
    store::{
        Conversation, ConversationPatch, ConversationSummary, HistoryQuery, ReasoningEffort, Store,
        Turn,
    },
};
use serde_json::Value;
use uuid::Uuid;

pub trait Chats {
    async fn create(&self, owner: Uuid, title: &str, model: &str)
        -> Result<Conversation, ApiError>;
    async fn rename(&self, owner: Uuid, id: Uuid, title: &str) -> Result<Conversation, ApiError>;
    /// Every chat the person keeps, archived or not, through history.
    async fn list(&self, owner: Uuid) -> Result<Vec<ConversationSummary>, ApiError>;
    #[allow(clippy::too_many_arguments)]
    async fn start_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
    ) -> Result<Turn, ApiError>;
    #[allow(clippy::too_many_arguments)]
    async fn start_turn_with_effort(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Turn, ApiError>;
    #[allow(clippy::too_many_arguments)]
    async fn start_turn_configured(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
        top_p: f64,
        requested_model: &str,
    ) -> Result<Turn, ApiError>;
    #[allow(clippy::too_many_arguments)]
    async fn finish_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        content: &str,
        status: &str,
        usage: Option<Value>,
        request_id: Option<&str>,
    ) -> Result<(), ApiError>;
    #[allow(clippy::too_many_arguments)]
    async fn finish_turn_with_reasoning(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        content: &str,
        reasoning_content: &str,
        status: &str,
        usage: Option<Value>,
        request_id: Option<&str>,
    ) -> Result<(), ApiError>;
    async fn interrupt_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        partial_content: &str,
    ) -> Result<(), ApiError>;
}

impl Chats for Store {
    async fn create(
        &self,
        owner: Uuid,
        title: &str,
        model: &str,
    ) -> Result<Conversation, ApiError> {
        self.create_with_effort(owner, title, model, ReasoningEffort::default())
            .await
    }

    async fn rename(&self, owner: Uuid, id: Uuid, title: &str) -> Result<Conversation, ApiError> {
        let patch = ConversationPatch {
            title: Some(title.into()),
            ..Default::default()
        };
        self.patch(owner, id, patch).await
    }

    async fn list(&self, owner: Uuid) -> Result<Vec<ConversationSummary>, ApiError> {
        let mut all = vec![];
        loop {
            let query = HistoryQuery {
                archived: Some("all".into()),
                limit: Some(100),
                offset: Some(all.len() as i64),
                ..Default::default()
            };
            let page = self.history(owner, query).await?;
            all.extend(page.conversations);
            if !page.has_more {
                return Ok(all);
            }
        }
    }

    async fn start_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
    ) -> Result<Turn, ApiError> {
        self.start_turn_with_effort(
            owner,
            id,
            content,
            model,
            system_prompt,
            temperature,
            max_tokens,
            None,
        )
        .await
    }

    async fn start_turn_with_effort(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Turn, ApiError> {
        self.start_turn_configured(
            owner,
            id,
            content,
            model,
            system_prompt,
            temperature,
            max_tokens,
            reasoning_effort,
            1.0,
            model,
        )
        .await
    }

    async fn start_turn_configured(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
        top_p: f64,
        requested_model: &str,
    ) -> Result<Turn, ApiError> {
        self.start_turn_attached(
            owner,
            id,
            content,
            model,
            system_prompt,
            temperature,
            max_tokens,
            reasoning_effort,
            top_p,
            requested_model,
            &[],
            false,
        )
        .await
    }

    async fn finish_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        content: &str,
        status: &str,
        usage: Option<Value>,
        request_id: Option<&str>,
    ) -> Result<(), ApiError> {
        self.finish_turn_with_reasoning(
            owner,
            id,
            generation_id,
            content,
            "",
            status,
            usage,
            request_id,
        )
        .await
    }

    async fn finish_turn_with_reasoning(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        content: &str,
        reasoning_content: &str,
        status: &str,
        usage: Option<Value>,
        request_id: Option<&str>,
    ) -> Result<(), ApiError> {
        self.finish_turn_detailed(
            owner,
            id,
            generation_id,
            content,
            reasoning_content,
            status,
            usage,
            request_id,
            None,
            None,
        )
        .await
    }

    async fn interrupt_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        partial_content: &str,
    ) -> Result<(), ApiError> {
        self.finish_turn(
            owner,
            id,
            generation_id,
            partial_content,
            "interrupted",
            None,
            None,
        )
        .await
    }
}

/// Links messages a test wrote by hand, as the path migration does for chats
/// written before every writer set the links.
pub async fn link_written_messages(pool: &sqlx::PgPool) {
    sqlx::raw_sql(include_str!("../../migrations/087_initialize_paths.sql"))
        .execute(pool)
        .await
        .unwrap();
}
