use std::{env, path::PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub telegram_bot_token: String,
    pub database_url: String,
    pub download_dir: PathBuf,
    pub yt_dlp_bin: String,
    pub yt_dlp_cookies: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let telegram_bot_token = env::var("TELEGRAM_BOT_TOKEN")
            .or_else(|_| env::var("TELOXIDE_TOKEN"))
            .or_else(|_| env::var("BOT_TOKEN"))
            .context("set TELEGRAM_BOT_TOKEN, TELOXIDE_TOKEN, or BOT_TOKEN")?;

        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://kiwi_cache.sqlite?mode=rwc".to_string());
        let download_dir = env::var("DOWNLOAD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("downloads"));
        let yt_dlp_bin = env::var("YT_DLP_BIN").unwrap_or_else(|_| "yt-dlp".to_string());
        let yt_dlp_cookies = env::var("YT_DLP_COOKIES")
            .ok()
            .or_else(|| {
                if std::path::Path::new("cookies.txt").exists() {
                    Some("cookies.txt".to_string())
                } else {
                    None
                }
            });

        Ok(Self {
            telegram_bot_token,
            database_url,
            download_dir,
            yt_dlp_bin,
            yt_dlp_cookies,
        })
    }
}
