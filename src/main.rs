mod config;
mod db;
mod handlers;
mod locales;
mod models;
mod services;

use std::sync::Arc;

use anyhow::Result;
use teloxide::prelude::*;

use crate::{
    config::Config,
    db::CacheDb,
    handlers::BotState,
    locales::I18n,
    services::downloader::YtDlp,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    pretty_env_logger::init();

    let config = Config::from_env()?;
    tokio::fs::create_dir_all(&config.download_dir).await?;

    let bot = Bot::new(config.telegram_bot_token.clone());
    let db = Arc::new(CacheDb::connect(&config.database_url).await?);
    db.migrate().await?;

    let i18n = Arc::new(I18n::new()?);
    let downloader = Arc::new(YtDlp::new(config.yt_dlp_bin, config.download_dir));
    let state = Arc::new(BotState::default());

    log::info!("Starting Kiwi Downloader bot");

    // Hugging Face Spaces health check dummy server
    std::thread::spawn(|| {
        use std::io::Write;
        if let Ok(listener) = std::net::TcpListener::bind("0.0.0.0:7860") {
            log::info!("Dummy health check server running on port 7860");
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        }
    });

    handlers::run(bot, db, i18n, downloader, state).await;

    Ok(())
}
