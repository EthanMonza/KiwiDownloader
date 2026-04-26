pub mod callbacks;
pub mod messages;

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Result;
use teloxide::{
    dptree,
    dispatching::{UpdateFilterExt, UpdateHandler},
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, User},
};
use tokio::{sync::Mutex, time::Instant};
use uuid::Uuid;

use crate::{
    db::CacheDb,
    locales::{I18n, SUPPORTED_LANGUAGES},
    services::downloader::{MediaInfo, YtDlp},
};

/// Maximum time a pending job stays in memory before automatic eviction.
const JOB_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct PendingJob {
    pub url: String,
    pub info: MediaInfo,
    pub language_code: String,
    pub user_id: u64,
}

#[derive(Debug, Clone)]
struct TimestampedJob {
    job: PendingJob,
    created_at: Instant,
}

#[derive(Default)]
pub struct BotState {
    pending: Mutex<HashMap<String, TimestampedJob>>,
}

impl BotState {
    pub async fn insert(&self, job: PendingJob) -> String {
        let token = Uuid::new_v4().simple().to_string();
        let mut map = self.pending.lock().await;

        // Evict expired entries to prevent unbounded growth.
        map.retain(|_, entry| entry.created_at.elapsed() < JOB_TTL);

        map.insert(
            token.clone(),
            TimestampedJob {
                job,
                created_at: Instant::now(),
            },
        );
        token
    }

    /// Retrieves a job without removing it (for read-only inspection).
    pub async fn get(&self, token: &str) -> Option<PendingJob> {
        self.pending
            .lock()
            .await
            .get(token)
            .filter(|entry| entry.created_at.elapsed() < JOB_TTL)
            .map(|entry| entry.job.clone())
    }

    /// Removes and returns a job (one-shot consumption).
    pub async fn take(&self, token: &str) -> Option<PendingJob> {
        self.pending
            .lock()
            .await
            .remove(token)
            .filter(|entry| entry.created_at.elapsed() < JOB_TTL)
            .map(|entry| entry.job)
    }
}

pub type HandlerResult = Result<()>;

pub async fn run(
    bot: Bot,
    db: Arc<CacheDb>,
    i18n: Arc<I18n>,
    downloader: Arc<YtDlp>,
    state: Arc<BotState>,
) {
    Dispatcher::builder(bot, schema())
        .dependencies(dptree::deps![db, i18n, downloader, state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

fn schema() -> UpdateHandler<anyhow::Error> {
    dptree::entry()
        .branch(Update::filter_message().endpoint(messages::handle_message))
        .branch(Update::filter_callback_query().endpoint(callbacks::handle_callback))
}

pub async fn resolve_language(
    db: &CacheDb,
    i18n: &I18n,
    user: Option<&User>,
) -> Result<String> {
    let Some(user) = user else {
        return Ok(i18n.normalize_language(None));
    };

    if let Some(saved) = db.get_user_language(user.id.0 as i64).await? {
        if i18n.is_supported(&saved) {
            return Ok(saved);
        }
    }

    Ok(i18n.normalize_language(user.language_code.as_deref()))
}

pub fn language_keyboard() -> InlineKeyboardMarkup {
    let rows = SUPPORTED_LANGUAGES
        .chunks(2)
        .map(|chunk| {
            chunk
                .iter()
                .map(|language| {
                    InlineKeyboardButton::callback(
                        language.name.to_string(),
                        format!("lang:{}", language.code),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    InlineKeyboardMarkup::new(rows)
}
