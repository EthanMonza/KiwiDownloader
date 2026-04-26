use std::{path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use teloxide::{
    payloads::{EditMessageTextSetters, SendAudioSetters, SendVideoSetters},
    prelude::*,
    types::{
        ChatId, FileId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, InputMedia,
        InputMediaPhoto, InputMediaVideo, Message, MessageId,
    },
};
use tokio::{sync::mpsc, time::Instant};

use crate::{
    db::{CacheDb, CachedFile},
    handlers::{resolve_language, BotState, HandlerResult},
    locales::I18n,
    models::{DownloadKind, MediaKind, VideoQuality},
    services::downloader::{DownloadedFile, DownloadedSet, ProgressEvent, YtDlp},
};

#[derive(Debug, Clone, Default)]
struct SendMetadata {
    title: Option<String>,
    performer: Option<String>,
}

pub async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    db: Arc<CacheDb>,
    i18n: Arc<I18n>,
    downloader: Arc<YtDlp>,
    state: Arc<BotState>,
) -> HandlerResult {
    bot.answer_callback_query(q.id.clone()).await?;

    let Some(data) = q.data.clone() else {
        return Ok(());
    };

    if let Some(language_code) = data.strip_prefix("lang:") {
        return handle_language_callback(bot, q, db, i18n, language_code).await;
    }

    if let Some(message) = q.regular_message() {
        if let Some((token, kind)) = parse_format_callback(&data) {
            return handle_format_callback(
                bot,
                message,
                &q,
                db,
                i18n,
                downloader,
                state,
                token,
                kind,
            )
            .await;
        }

        if let Some((token, quality)) = parse_quality_callback(&data) {
            return handle_quality_callback(
                bot,
                message,
                &q,
                db,
                i18n,
                downloader,
                state,
                token,
                quality,
            )
            .await;
        }
    }

    Ok(())
}

async fn handle_language_callback(
    bot: Bot,
    q: CallbackQuery,
    db: Arc<CacheDb>,
    i18n: Arc<I18n>,
    language_code: &str,
) -> HandlerResult {
    if !i18n.is_supported(language_code) {
        return Ok(());
    }

    db.set_user_language(q.from.id.0 as i64, language_code).await?;

    let text = i18n.t(
        language_code,
        "language-saved",
        &[("language", i18n.language_name(language_code).to_string())],
    );

    if let Some(message) = q.regular_message() {
        bot.edit_message_text(message.chat.id, message.id, text).await?;
    }

    Ok(())
}

async fn handle_format_callback(
    bot: Bot,
    message: &Message,
    q: &CallbackQuery,
    db: Arc<CacheDb>,
    i18n: Arc<I18n>,
    downloader: Arc<YtDlp>,
    state: Arc<BotState>,
    token: &str,
    kind: DownloadKind,
) -> HandlerResult {
    let Some(job) = state.get(token).await else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            i18n.t(&resolve_language(&db, &i18n, Some(&q.from)).await?, "expired", &[]),
        )
        .await?;
        return Ok(());
    };

    if job.user_id != q.from.id.0 {
        bot.send_message(
            message.chat.id,
            i18n.t(&job.language_code, "not-your-request", &[]),
        )
        .await?;
        return Ok(());
    }

    match kind {
        DownloadKind::Audio => {
            if !job.info.has_audio {
                bot.edit_message_text(
                    message.chat.id,
                    message.id,
                    i18n.t(&job.language_code, "audio-not-available", &[]),
                )
                .await?;
                return Ok(());
            }

            process_download(
                bot,
                message.chat.id,
                message.id,
                db,
                i18n,
                downloader,
                job.url,
                DownloadKind::Audio,
                "mp3".to_string(),
                None,
                job.language_code,
                false,
                1,
                SendMetadata {
                    title: job.info.title.clone(),
                    performer: job.info.uploader.clone(),
                },
            )
            .await?;
        }
        DownloadKind::Video => {
            let qualities = job.info.qualities.clone();
            bot.edit_message_text(
                message.chat.id,
                message.id,
                i18n.t(
                    &job.language_code,
                    "choose-quality",
                    &[(
                        "qualities",
                        qualities
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    )],
                ),
            )
            .reply_markup(quality_keyboard(token, &qualities))
            .await?;
        }
    }

    Ok(())
}

async fn handle_quality_callback(
    bot: Bot,
    message: &Message,
    q: &CallbackQuery,
    db: Arc<CacheDb>,
    i18n: Arc<I18n>,
    downloader: Arc<YtDlp>,
    state: Arc<BotState>,
    token: &str,
    quality: VideoQuality,
) -> HandlerResult {
    let Some(job) = state.take(token).await else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            i18n.t(&resolve_language(&db, &i18n, Some(&q.from)).await?, "expired", &[]),
        )
        .await?;
        return Ok(());
    };

    if job.user_id != q.from.id.0 {
        bot.send_message(
            message.chat.id,
            i18n.t(&job.language_code, "not-your-request", &[]),
        )
        .await?;
        return Ok(());
    }

    process_download(
        bot,
        message.chat.id,
        message.id,
        db,
        i18n,
        downloader,
        job.url,
        DownloadKind::Video,
        quality.cache_key(),
        Some(quality),
        job.language_code,
        job.info.is_carousel(),
        if job.info.is_carousel() {
            job.info.entries_count.max(1)
        } else {
            1
        },
        SendMetadata {
            title: job.info.title.clone(),
            performer: job.info.uploader.clone(),
        },
    )
    .await?;

    Ok(())
}

async fn process_download(
    bot: Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db: Arc<CacheDb>,
    i18n: Arc<I18n>,
    downloader: Arc<YtDlp>,
    url: String,
    download_kind: DownloadKind,
    quality_key: String,
    quality: Option<VideoQuality>,
    language_code: String,
    carousel: bool,
    expected_cache_items: usize,
    metadata: SendMetadata,
) -> Result<()> {
    bot.edit_message_text(chat_id, message_id, i18n.t(&language_code, "checking-cache", &[]))
        .await?;

    let cached = db
        .cached_files(&url, download_kind.as_str(), &quality_key)
        .await?;
    if cached.len() >= expected_cache_items {
        bot.edit_message_text(chat_id, message_id, i18n.t(&language_code, "cache-hit", &[]))
            .await?;
        send_cached(&bot, chat_id, &cached).await?;
        bot.edit_message_text(chat_id, message_id, i18n.t(&language_code, "done", &[]))
            .await?;
        return Ok(());
    }

    let (progress_tx, progress_rx) = mpsc::channel(32);
    let progress_task = tokio::spawn(update_progress(
        bot.clone(),
        chat_id,
        message_id,
        i18n.clone(),
        language_code.clone(),
        progress_rx,
    ));

    let downloaded = match download_kind {
        DownloadKind::Audio => downloader.download_audio(&url, progress_tx).await,
        DownloadKind::Video => {
            downloader
                .download_video(&url, quality.unwrap_or(VideoQuality::Best), carousel, progress_tx)
                .await
        }
    };

    progress_task.abort();

    let downloaded = match downloaded {
        Ok(downloaded) => downloaded,
        Err(error) => {
            bot.edit_message_text(
                chat_id,
                message_id,
                i18n.t(&language_code, "error", &[("details", error.to_string())]),
            )
            .await?;
            return Ok(());
        }
    };

    bot.edit_message_text(chat_id, message_id, i18n.t(&language_code, "uploading", &[]))
        .await?;

    let sent_messages = send_downloaded(&bot, chat_id, &downloaded, download_kind, &metadata).await?;
    cache_sent_messages(
        &db,
        &url,
        download_kind.as_str(),
        &quality_key,
        &downloaded.files,
        &sent_messages,
    )
    .await?;

    downloaded.cleanup().await;

    bot.edit_message_text(chat_id, message_id, i18n.t(&language_code, "done", &[]))
        .await?;

    Ok(())
}

async fn update_progress(
    bot: Bot,
    chat_id: ChatId,
    message_id: MessageId,
    i18n: Arc<I18n>,
    language_code: String,
    mut progress_rx: mpsc::Receiver<ProgressEvent>,
) {
    let mut last_update = Instant::now() - Duration::from_secs(5);
    let mut last_percent = 0_u8;

    while let Some(event) = progress_rx.recv().await {
        let ProgressEvent::Percent(percent) = event;
        let rounded = (percent.clamp(0.0, 100.0)).round() as u8;

        if rounded == last_percent && last_update.elapsed() < Duration::from_secs(2) {
            continue;
        }
        if last_update.elapsed() < Duration::from_millis(900) && rounded < 100 {
            continue;
        }

        last_percent = rounded;
        last_update = Instant::now();
        let text = i18n.t(
            &language_code,
            "downloading",
            &[
                ("percent", rounded.to_string()),
                ("bar", progress_bar(rounded)),
            ],
        );

        if let Err(error) = bot.edit_message_text(chat_id, message_id, text).await {
            log::debug!("failed to update progress: {error}");
        }
    }
}

fn progress_bar(percent: u8) -> String {
    let filled = (percent as usize * 12) / 100;
    let empty = 12 - filled;
    format!("[{}{}]", "#".repeat(filled), "-".repeat(empty))
}

async fn send_cached(bot: &Bot, chat_id: ChatId, cached: &[CachedFile]) -> Result<Vec<Message>> {
    if cached.len() == 1 {
        return Ok(vec![send_single_cached(bot, chat_id, &cached[0]).await?]);
    }

    if !cached.iter().all(|file| supports_media_group(file.kind())) {
        let mut messages = Vec::new();
        for file in cached {
            messages.push(send_single_cached(bot, chat_id, file).await?);
        }
        return Ok(messages);
    }

    send_cached_media_groups(bot, chat_id, cached).await
}

async fn send_cached_media_groups(
    bot: &Bot,
    chat_id: ChatId,
    cached: &[CachedFile],
) -> Result<Vec<Message>> {
    let mut messages = Vec::new();

    for chunk in cached.chunks(10) {
        if chunk.len() == 1 {
            messages.push(send_single_cached(bot, chat_id, &chunk[0]).await?);
            continue;
        }

        let media = chunk
            .iter()
            .filter_map(|file| input_media_from_file_id(file.kind(), &file.file_id))
            .collect::<Vec<_>>();
        if media.len() >= 2 {
            messages.extend(bot.send_media_group(chat_id, media).await?);
        } else {
            // Telegram requires at least 2 items for sendMediaGroup.
            for file in chunk {
                messages.push(send_single_cached(bot, chat_id, file).await?);
            }
        }
    }

    Ok(messages)
}

async fn send_downloaded(
    bot: &Bot,
    chat_id: ChatId,
    downloaded: &DownloadedSet,
    download_kind: DownloadKind,
    metadata: &SendMetadata,
) -> Result<Vec<Message>> {
    if download_kind == DownloadKind::Audio
        || !downloaded
            .files
            .iter()
            .all(|file| supports_media_group(file.media_kind))
    {
        let mut messages = Vec::new();
        for file in &downloaded.files {
            messages.push(send_single_downloaded(
                bot,
                chat_id,
                file,
                download_kind,
                metadata,
            )
            .await?);
        }
        return Ok(messages);
    }

    if downloaded.files.len() == 1 {
        let message = send_single_downloaded(
            bot,
            chat_id,
            &downloaded.files[0],
            download_kind,
            metadata,
        )
        .await?;
        return Ok(vec![message]);
    }

    let mut messages = Vec::new();
    for chunk in downloaded.files.chunks(10) {
        if chunk.len() == 1 {
            messages.push(
                send_single_downloaded(bot, chat_id, &chunk[0], download_kind, metadata).await?,
            );
            continue;
        }

        let media = chunk
            .iter()
            .filter_map(|file| input_media_from_path(file.media_kind, &file.path))
            .collect::<Vec<_>>();

        if media.len() >= 2 {
            messages.extend(bot.send_media_group(chat_id, media).await?);
        } else {
            // Telegram requires at least 2 items for sendMediaGroup.
            for file in chunk {
                messages.push(
                    send_single_downloaded(bot, chat_id, file, download_kind, metadata).await?,
                );
            }
        }
    }

    Ok(messages)
}

async fn send_single_cached(bot: &Bot, chat_id: ChatId, file: &CachedFile) -> Result<Message> {
    let input = InputFile::file_id(FileId(file.file_id.clone()));
    let message = match file.kind() {
        MediaKind::Audio => bot.send_audio(chat_id, input).await?,
        MediaKind::Video => bot.send_video(chat_id, input).supports_streaming(true).await?,
        MediaKind::Photo => bot.send_photo(chat_id, input).await?,
        MediaKind::Document => bot.send_document(chat_id, input).await?,
    };

    Ok(message)
}

async fn send_single_downloaded(
    bot: &Bot,
    chat_id: ChatId,
    file: &DownloadedFile,
    download_kind: DownloadKind,
    metadata: &SendMetadata,
) -> Result<Message> {
    let input = InputFile::file(file.path.clone());
    let message = match download_kind {
        DownloadKind::Audio => {
            let mut request = bot.send_audio(chat_id, input);
            if let Some(title) = metadata.title.clone() {
                request = request.title(title);
            }
            if let Some(performer) = metadata.performer.clone() {
                request = request.performer(performer);
            }
            request.await?
        }
        DownloadKind::Video => match file.media_kind {
            MediaKind::Photo => bot.send_photo(chat_id, input).await?,
            MediaKind::Video => {
                let mut request = bot.send_video(chat_id, input).supports_streaming(true);
                if let Some(title) = metadata.title.clone() {
                    request = request.caption(title);
                }
                request.await?
            }
            _ => bot.send_document(chat_id, input).await?,
        },
    };

    Ok(message)
}

fn supports_media_group(kind: MediaKind) -> bool {
    matches!(kind, MediaKind::Photo | MediaKind::Video)
}

fn input_media_from_path(kind: MediaKind, path: &Path) -> Option<InputMedia> {
    let input = InputFile::file(path.to_path_buf());
    match kind {
        MediaKind::Photo => Some(InputMedia::Photo(InputMediaPhoto::new(input))),
        MediaKind::Video => Some(InputMedia::Video(
            InputMediaVideo::new(input).supports_streaming(true),
        )),
        _ => None,
    }
}

fn input_media_from_file_id(kind: MediaKind, file_id: &str) -> Option<InputMedia> {
    let input = InputFile::file_id(FileId(file_id.to_string()));
    match kind {
        MediaKind::Photo => Some(InputMedia::Photo(InputMediaPhoto::new(input))),
        MediaKind::Video => Some(InputMedia::Video(
            InputMediaVideo::new(input).supports_streaming(true),
        )),
        _ => None,
    }
}

async fn cache_sent_messages(
    db: &CacheDb,
    url: &str,
    download_kind: &str,
    quality: &str,
    files: &[DownloadedFile],
    sent_messages: &[Message],
) -> Result<()> {
    for (index, message) in sent_messages.iter().enumerate() {
        let Some((file_type, file_id, file_unique_id)) = extract_file_id(message) else {
            continue;
        };

        let item_index = files
            .get(index)
            .map(|file| file.item_index)
            .unwrap_or(index as i64);

        db.upsert_cached_file(
            url,
            download_kind,
            quality,
            item_index,
            file_type,
            &file_id,
            file_unique_id.as_deref(),
        )
        .await?;
    }

    Ok(())
}

fn extract_file_id(message: &Message) -> Option<(MediaKind, String, Option<String>)> {
    if let Some(audio) = message.audio() {
        return Some((
            MediaKind::Audio,
            audio.file.id.to_string(),
            Some(audio.file.unique_id.to_string()),
        ));
    }

    if let Some(video) = message.video() {
        return Some((
            MediaKind::Video,
            video.file.id.to_string(),
            Some(video.file.unique_id.to_string()),
        ));
    }

    if let Some(photo_sizes) = message.photo() {
        let photo = photo_sizes.last()?;
        return Some((
            MediaKind::Photo,
            photo.file.id.to_string(),
            Some(photo.file.unique_id.to_string()),
        ));
    }

    if let Some(document) = message.document() {
        return Some((
            MediaKind::Document,
            document.file.id.to_string(),
            Some(document.file.unique_id.to_string()),
        ));
    }

    None
}

fn quality_keyboard(token: &str, qualities: &[VideoQuality]) -> InlineKeyboardMarkup {
    let rows = qualities
        .chunks(3)
        .map(|chunk| {
            chunk
                .iter()
                .map(|quality| {
                    InlineKeyboardButton::callback(
                        quality.to_string(),
                        format!("quality:{token}:{}", quality.callback_value()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    InlineKeyboardMarkup::new(rows)
}

fn parse_format_callback(data: &str) -> Option<(&str, DownloadKind)> {
    let mut parts = data.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("format"), Some(token), Some("audio"), None) => Some((token, DownloadKind::Audio)),
        (Some("format"), Some(token), Some("video"), None) => Some((token, DownloadKind::Video)),
        _ => None,
    }
}

fn parse_quality_callback(data: &str) -> Option<(&str, VideoQuality)> {
    let mut parts = data.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("quality"), Some(token), Some(quality), None) => {
            VideoQuality::from_callback(quality).map(|quality| (token, quality))
        }
        _ => None,
    }
}
