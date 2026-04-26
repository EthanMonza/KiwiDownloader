use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadKind {
    Audio,
    Video,
}

impl DownloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Video,
    Photo,
    Document,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Photo => "photo",
            Self::Document => "document",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "audio" => Self::Audio,
            "video" => Self::Video,
            "photo" => Self::Photo,
            _ => Self::Document,
        }
    }

    pub fn from_extension(extension: &str) -> Self {
        match extension.to_ascii_lowercase().as_str() {
            "mp3" | "m4a" | "aac" | "opus" | "ogg" => Self::Audio,
            "mp4" | "m4v" | "mov" | "webm" | "mkv" => Self::Video,
            "jpg" | "jpeg" | "png" | "webp" => Self::Photo,
            _ => Self::Document,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoQuality {
    Best,
    Height(u32),
}

impl VideoQuality {
    pub fn cache_key(self) -> String {
        match self {
            Self::Best => "best".to_string(),
            Self::Height(height) => format!("{height}p"),
        }
    }

    pub fn callback_value(self) -> String {
        match self {
            Self::Best => "best".to_string(),
            Self::Height(height) => height.to_string(),
        }
    }

    pub fn from_callback(value: &str) -> Option<Self> {
        if value == "best" {
            return Some(Self::Best);
        }

        value.parse::<u32>().ok().map(Self::Height)
    }

    pub fn format_selector(self) -> String {
        match self {
            Self::Best => "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/best".to_string(),
            Self::Height(height) => {
                format!(
                    "bv*[height<={height}][ext=mp4]+ba[ext=m4a]/b[height<={height}][ext=mp4]/best[height<={height}]/best"
                )
            }
        }
    }
}

impl fmt::Display for VideoQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Best => write!(f, "best"),
            Self::Height(height) => write!(f, "{height}p"),
        }
    }
}
