use anyhow::Result;
use sqlx::{sqlite::SqlitePoolOptions, FromRow, Row, SqlitePool};

use crate::models::MediaKind;

#[derive(Debug, Clone, FromRow)]
pub struct CachedFile {
    pub url: String,
    pub media_kind: String,
    pub quality: String,
    pub item_index: i64,
    pub file_type: String,
    pub file_id: String,
    pub file_unique_id: Option<String>,
}

impl CachedFile {
    pub fn kind(&self) -> MediaKind {
        MediaKind::from_str(&self.file_type)
    }
}

#[derive(Clone)]
pub struct CacheDb {
    pool: SqlitePool,
}

impl CacheDb {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;

        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS telegram_file_cache (
                url TEXT NOT NULL,
                media_kind TEXT NOT NULL,
                quality TEXT NOT NULL,
                item_index INTEGER NOT NULL DEFAULT 0,
                file_type TEXT NOT NULL,
                file_id TEXT NOT NULL,
                file_unique_id TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (url, media_kind, quality, item_index)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS user_languages (
                user_id INTEGER PRIMARY KEY,
                language_code TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn cached_files(
        &self,
        url: &str,
        media_kind: &str,
        quality: &str,
    ) -> Result<Vec<CachedFile>> {
        let files = sqlx::query_as::<_, CachedFile>(
            r#"
            SELECT url, media_kind, quality, item_index, file_type, file_id, file_unique_id
            FROM telegram_file_cache
            WHERE url = ? AND media_kind = ? AND quality = ?
            ORDER BY item_index ASC
            "#,
        )
        .bind(url)
        .bind(media_kind)
        .bind(quality)
        .fetch_all(&self.pool)
        .await?;

        Ok(files)
    }

    pub async fn upsert_cached_file(
        &self,
        url: &str,
        media_kind: &str,
        quality: &str,
        item_index: i64,
        file_type: MediaKind,
        file_id: &str,
        file_unique_id: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO telegram_file_cache (
                url, media_kind, quality, item_index, file_type, file_id, file_unique_id
            )
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(url, media_kind, quality, item_index)
            DO UPDATE SET
                file_type = excluded.file_type,
                file_id = excluded.file_id,
                file_unique_id = excluded.file_unique_id,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(url)
        .bind(media_kind)
        .bind(quality)
        .bind(item_index)
        .bind(file_type.as_str())
        .bind(file_id)
        .bind(file_unique_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_user_language(&self, user_id: i64) -> Result<Option<String>> {
        let row = sqlx::query("SELECT language_code FROM user_languages WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|row| row.get::<String, _>("language_code")))
    }

    pub async fn set_user_language(&self, user_id: i64, language_code: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO user_languages (user_id, language_code)
            VALUES (?, ?)
            ON CONFLICT(user_id)
            DO UPDATE SET
                language_code = excluded.language_code,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(user_id)
        .bind(language_code)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
