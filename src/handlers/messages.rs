use std::sync::Arc;

use teloxide::{
    payloads::EditMessageTextSetters,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

use crate::{
    db::CacheDb,
    handlers::{language_keyboard, resolve_language, BotState, HandlerResult, PendingJob},
    locales::I18n,
    services::downloader::{Source, YtDlp},
};

pub async fn handle_message(
    bot: Bot,
    msg: Message,
    db: Arc<CacheDb>,
    i18n: Arc<I18n>,
    downloader: Arc<YtDlp>,
    state: Arc<BotState>,
) -> HandlerResult {
    let language_code = resolve_language(&db, &i18n, msg.from.as_ref()).await?;
    let Some(text) = msg.text() else {
        bot.send_message(msg.chat.id, i18n.t(&language_code, "not-a-link", &[]))
            .await?;
        return Ok(());
    };

    if text.starts_with("/start") {
        bot.send_message(msg.chat.id, i18n.t(&language_code, "welcome", &[]))
            .reply_markup(language_keyboard())
            .await?;
        return Ok(());
    }

    if text.starts_with("/language") || text.starts_with("/lang") {
        bot.send_message(msg.chat.id, i18n.t(&language_code, "language-menu", &[]))
            .reply_markup(language_keyboard())
            .await?;
        return Ok(());
    }

    let Some(url) = extract_url(text) else {
        bot.send_message(msg.chat.id, i18n.t(&language_code, "not-a-link", &[]))
            .await?;
        return Ok(());
    };

    if Source::from_url(&url).is_err() {
        bot.send_message(msg.chat.id, i18n.t(&language_code, "unsupported-url", &[]))
            .await?;
        return Ok(());
    }

    let status = bot
        .send_message(msg.chat.id, i18n.t(&language_code, "analyzing", &[]))
        .await?;

    match downloader.probe(&url).await {
        Ok(info) => {
            let Some(user) = msg.from.as_ref() else {
                bot.edit_message_text(
                    msg.chat.id,
                    status.id,
                    i18n.t(&language_code, "private-only", &[]),
                )
                .await?;
                return Ok(());
            };

            let token = state
                .insert(PendingJob {
                    url,
                    info: info.clone(),
                    language_code: language_code.clone(),
                    user_id: user.id.0,
                })
                .await;

            bot.edit_message_text(
                msg.chat.id,
                status.id,
                i18n.t(
                    &language_code,
                    "choose-format",
                    &[("title", info.title_or_url())],
                ),
            )
            .reply_markup(format_keyboard(&token, &i18n, &language_code))
            .await?;
        }
        Err(error) => {
            bot.edit_message_text(
                msg.chat.id,
                status.id,
                i18n.t(
                    &language_code,
                    "error",
                    &[("details", error.to_string())],
                ),
            )
            .await?;
        }
    }

    Ok(())
}

fn extract_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|part| part.starts_with("http://") || part.starts_with("https://"))
        .map(|part| {
            part.trim_matches(|character: char| {
                matches!(character, ')' | '(' | '[' | ']' | '{' | '}' | ',' | ';')
            })
            .to_string()
        })
}

fn format_keyboard(
    token: &str,
    i18n: &I18n,
    language_code: &str,
) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            i18n.t(language_code, "button-video", &[]),
            format!("format:{token}:video"),
        ),
        InlineKeyboardButton::callback(
            i18n.t(language_code, "button-audio", &[]),
            format!("format:{token}:audio"),
        ),
    ]])
}
