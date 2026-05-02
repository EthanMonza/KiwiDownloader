use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tokio::{sync::mpsc, task};
use url::Url;
use uuid::Uuid;

use crate::models::{MediaKind, VideoQuality};

pub type ProgressSender = mpsc::Sender<ProgressEvent>;

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Percent(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    YouTube,
    TikTok,
    Instagram,
    Pinterest,
    Spotify,
}

impl Source {
    pub fn from_url(raw_url: &str) -> Result<Self> {
        let url = Url::parse(raw_url).context("invalid URL")?;
        let host = url
            .host_str()
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| anyhow!("URL has no host"))?;

        if host == "youtu.be" || is_domain_or_subdomain(&host, "youtube.com") {
            return Ok(Self::YouTube);
        }
        if is_domain_or_subdomain(&host, "tiktok.com") {
            return Ok(Self::TikTok);
        }
        if is_domain_or_subdomain(&host, "instagram.com") {
            return Ok(Self::Instagram);
        }
        if host == "pin.it" || is_domain_or_subdomain(&host, "pinterest.com") {
            return Ok(Self::Pinterest);
        }
        if is_domain_or_subdomain(&host, "spotify.com") {
            return Ok(Self::Spotify);
        }

        bail!("unsupported media source")
    }
}

fn is_domain_or_subdomain(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub source: Source,
    pub webpage_url: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub duration_seconds: Option<u64>,
    pub qualities: Vec<VideoQuality>,
    pub entries_count: usize,
    pub has_audio: bool,
}

impl MediaInfo {
    pub fn is_carousel(&self) -> bool {
        self.source == Source::Instagram && self.entries_count > 1
    }

    pub fn title_or_url(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| self.webpage_url.clone())
    }
}

#[derive(Debug, Clone)]
pub struct DownloadedFile {
    pub path: PathBuf,
    pub media_kind: MediaKind,
    pub item_index: i64,
}

#[derive(Debug, Clone)]
pub struct DownloadedSet {
    pub work_dir: PathBuf,
    pub files: Vec<DownloadedFile>,
}

impl DownloadedSet {
    pub async fn cleanup(self) {
        if let Err(error) = tokio::fs::remove_dir_all(&self.work_dir).await {
            log::warn!("failed to cleanup {:?}: {error}", self.work_dir);
        }
    }
}

#[derive(Debug, Deserialize)]
struct YtDlpInfo {
    id: Option<String>,
    title: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    height: Option<u32>,
    vcodec: Option<String>,
    acodec: Option<String>,
    webpage_url: Option<String>,
    original_url: Option<String>,
    #[serde(default)]
    formats: Vec<YtDlpFormat>,
    #[serde(default)]
    entries: Vec<Option<YtDlpInfo>>,
}

#[derive(Debug, Deserialize)]
struct YtDlpFormat {
    height: Option<u32>,
    vcodec: Option<String>,
    acodec: Option<String>,
}

impl YtDlpFormat {
    fn has_video(&self) -> bool {
        self.vcodec
            .as_deref()
            .is_some_and(|codec| !codec.eq_ignore_ascii_case("none"))
    }

    fn has_audio(&self) -> bool {
        self.acodec
            .as_deref()
            .is_some_and(|codec| !codec.eq_ignore_ascii_case("none"))
    }
}

#[derive(Clone)]
pub struct YtDlp {
    bin: String,
    download_root: PathBuf,
    cookies: Option<String>,
}

impl YtDlp {
    pub fn new(
        bin: String,
        download_root: PathBuf,
        cookies: Option<String>,
    ) -> Self {
        Self {
            bin,
            download_root,
            cookies,
        }
    }

    pub async fn probe(&self, raw_url: &str) -> Result<MediaInfo> {
        let mut source = Source::from_url(raw_url)?;
        let mut url = raw_url.trim().to_string();
        
        if source == Source::Spotify {
            let title = fetch_spotify_title(&url).await?;
            url = format!("ytsearch1:{}", title);
            // After re-mapping to YouTube search, we treat it as YouTube
            source = Source::YouTube;
        }

        let bin = self.bin.clone();
        let cookies = self.cookies.clone();

        let raw = task::spawn_blocking(move || -> Result<YtDlpInfo> {
            let mut last_error = String::new();
            
            for _ in 0..3 {
                let mut command = Command::new(&bin);
                command.args([
                    "--dump-single-json", 
                    "--no-warnings", 
                    "--skip-download",
                    "--extractor-args",
                    "youtube:player_client=ios",
                ]);
                
                if let Some(path) = &cookies {
                    command.arg("--cookies");
                    command.arg(path);
                }

                if source != Source::Instagram {
                    command.arg("--no-playlist");
                }
                command.arg(&url);

                let output = command.output().context("failed to start yt-dlp")?;
                if output.status.success() {
                    return serde_json::from_slice::<YtDlpInfo>(&output.stdout)
                        .context("failed to parse yt-dlp JSON");
                }
                
                last_error = String::from_utf8_lossy(&output.stderr).trim().to_string();
                std::thread::sleep(std::time::Duration::from_millis(500));
            }

            bail!("yt-dlp metadata probe failed: {}", last_error)
        })
        .await??;

        Ok(media_info_from_raw(raw_url, source, raw))
    }

    pub async fn download_audio(
        &self,
        raw_url: &str,
        progress: ProgressSender,
    ) -> Result<DownloadedSet> {
        let source = Source::from_url(raw_url)?;
        let mut url = raw_url.to_string();

        if source == Source::Spotify {
            let title = fetch_spotify_title(&url).await?;
            url = format!("ytsearch1:{}", title);
        }

        let (work_dir, mut args) = self.base_download_args().await?;
        args.extend([
            "--extract-audio".to_string(),
            "--audio-format".to_string(),
            "mp3".to_string(),
            "--audio-quality".to_string(),
            "0".to_string(),
        ]);
        if source != Source::Instagram && source != Source::Spotify {
            args.push("--no-playlist".to_string());
        }
        args.push(url);

        self.run_download(work_dir, args, progress).await
    }

    pub async fn download_video(
        &self,
        raw_url: &str,
        quality: VideoQuality,
        carousel: bool,
        progress: ProgressSender,
    ) -> Result<DownloadedSet> {
        let source = Source::from_url(raw_url)?;
        let mut url = raw_url.to_string();

        if source == Source::Spotify {
            let title = fetch_spotify_title(&url).await?;
            url = format!("ytsearch1:{}", title);
        }

        let (work_dir, mut args) = self.base_download_args().await?;

        if carousel {
            // Instagram carousels: skip format selector so yt-dlp picks the
            // native format for each slide (may be photos or videos).
            args.extend([
                "--merge-output-format".to_string(),
                "mp4".to_string(),
                url,
            ]);
        } else if source == Source::Pinterest {
            // Pinterest serves single-format media; strict format selectors
            // fail with "Requested format is not available". Let yt-dlp pick.
            args.extend([
                "--merge-output-format".to_string(),
                "mp4".to_string(),
                "--no-playlist".to_string(),
                url,
            ]);
        } else {
            args.extend([
                "--format".to_string(),
                quality.format_selector(),
                "--merge-output-format".to_string(),
                "mp4".to_string(),
                "--no-playlist".to_string(),
                url,
            ]);
        }

        self.run_download(work_dir, args, progress).await
    }

    async fn base_download_args(&self) -> Result<(PathBuf, Vec<String>)> {
        let work_dir = self.next_work_dir();
        tokio::fs::create_dir_all(&work_dir).await?;
        let template = work_dir
            .join("%(playlist_index)s-%(id)s.%(ext)s")
            .to_string_lossy()
            .into_owned();

        let mut args = vec![
            "--no-warnings".to_string(),
            "--no-colors".to_string(),
            "--newline".to_string(),
            "--progress".to_string(),
            "--progress-template".to_string(),
            "download:%(progress._percent_str)s".to_string(),
            "--restrict-filenames".to_string(),
            "-o".to_string(),
            template,
            "--extractor-args".to_string(),
            "youtube:player_client=ios".to_string(),
        ];

        if let Some(path) = &self.cookies {
            args.push("--cookies".to_string());
            args.push(path.clone());
        }

        Ok((work_dir, args))
    }

    async fn run_download(
        &self,
        work_dir: PathBuf,
        args: Vec<String>,
        progress: ProgressSender,
    ) -> Result<DownloadedSet> {
        let bin = self.bin.clone();

        task::spawn_blocking(move || -> Result<DownloadedSet> {
            run_yt_dlp_with_progress(&bin, &args, progress)?;
            let mut files = collect_downloaded_files(&work_dir)?;
            files.sort_by(|left, right| left.path.cmp(&right.path));

            if files.is_empty() {
                bail!("yt-dlp finished but produced no media files");
            }

            Ok(DownloadedSet { work_dir, files })
        })
        .await?
    }

    fn next_work_dir(&self) -> PathBuf {
        self.download_root.join(Uuid::new_v4().to_string())
    }
}

fn media_info_from_raw(input_url: &str, source: Source, raw: YtDlpInfo) -> MediaInfo {
    let mut heights = BTreeSet::new();
    let mut has_audio = false;

    collect_formats(&raw, &mut heights, &mut has_audio);

    let mut qualities: Vec<_> = heights
        .into_iter()
        .rev()
        .take(8)
        .map(VideoQuality::Height)
        .collect();
    if qualities.is_empty() {
        qualities.push(VideoQuality::Best);
    }

    MediaInfo {
        source,
        webpage_url: raw
            .webpage_url
            .or(raw.original_url)
            .unwrap_or_else(|| input_url.to_string()),
        title: raw.title.or(raw.id),
        uploader: raw.uploader,
        duration_seconds: raw.duration.map(|duration| duration.round() as u64),
        qualities,
        entries_count: raw.entries.iter().flatten().count(),
        has_audio,
    }
}

fn collect_formats(raw: &YtDlpInfo, heights: &mut BTreeSet<u32>, has_audio: &mut bool) {
    if raw
        .acodec
        .as_deref()
        .is_some_and(|codec| !codec.eq_ignore_ascii_case("none"))
    {
        *has_audio = true;
    }

    if raw
        .vcodec
        .as_deref()
        .is_some_and(|codec| !codec.eq_ignore_ascii_case("none"))
    {
        if let Some(height) = raw.height.filter(|height| *height > 0) {
            heights.insert(height);
        }
    }

    for format in &raw.formats {
        if format.has_audio() {
            *has_audio = true;
        }

        if format.has_video() {
            if let Some(height) = format.height.filter(|height| *height > 0) {
                heights.insert(height);
            }
        }
    }

    for entry in raw.entries.iter().flatten() {
        collect_formats(entry, heights, has_audio);
    }
}

// `output_dir_from_args` removed — work directory is now returned directly
// from `base_download_args()` to avoid fragile reverse-engineering from args.

fn run_yt_dlp_with_progress(
    bin: &str,
    args: &[String],
    progress: ProgressSender,
) -> Result<()> {
    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start yt-dlp")?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture yt-dlp stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture yt-dlp stderr"))?;

    let stderr_reader = thread::spawn(move || {
        let mut output = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut output);
        output
    });

    for line in BufReader::new(stdout).lines() {
        let line = line.context("failed to read yt-dlp progress")?;
        if let Some(percent) = parse_percent(&line) {
            let _ = progress.blocking_send(ProgressEvent::Percent(percent));
        }
    }

    let status = child.wait().context("failed to wait for yt-dlp")?;
    let stderr = stderr_reader
        .join()
        .unwrap_or_else(|_| "failed to join stderr reader".to_string());

    if !status.success() {
        bail!("yt-dlp download failed: {}", stderr.trim());
    }

    Ok(())
}

fn parse_percent(line: &str) -> Option<f32> {
    let percent_index = line.rfind('%')?;
    let before_percent = &line[..percent_index];
    let start = before_percent
        .rfind(|character: char| !(character.is_ascii_digit() || character == '.'))
        .map(|index| index + 1)
        .unwrap_or(0);

    before_percent[start..].trim().parse::<f32>().ok()
}

fn collect_downloaded_files(dir: &Path) -> Result<Vec<DownloadedFile>> {
    let mut paths = Vec::new();
    collect_files_recursive(dir, &mut paths)?;

    let files = paths
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("");
            let media_kind = MediaKind::from_extension(extension);
            DownloadedFile {
                path,
                media_kind,
                item_index: index as i64,
            }
        })
        .collect();

    Ok(files)
}

fn collect_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, files)?;
            continue;
        }

        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(extension.as_str(), "part" | "ytdl" | "tmp" | "json") {
            continue;
        }

        if entry.metadata()?.len() > 0 {
            files.push(path);
        }
    }

    Ok(())
}

async fn fetch_spotify_title(url: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://open.spotify.com/oembed")
        .query(&[("url", url)])
        .send()
        .await
        .context("failed to fetch Spotify oEmbed")?;

    if !response.status().is_success() {
        bail!("Spotify oEmbed returned error: {}", response.status());
    }

    let data: serde_json::Value = response.json().await.context("failed to parse Spotify oEmbed JSON")?;
    let title = data["title"]
        .as_str()
        .ok_or_else(|| anyhow!("Spotify oEmbed JSON missing title"))?;

    Ok(title.to_string())
}
