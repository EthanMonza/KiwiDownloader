FROM rust:1.95-slim-bookworm as builder

WORKDIR /usr/src/app

# Cache dependencies by building a dummy project first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src target/release/deps/kiwi_downloader* target/release/kiwi-downloader* target/release/.fingerprint/kiwi_downloader*

# Copy the actual source code and build
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim

# Install ffmpeg, python3 (required by yt-dlp), curl and ca-certificates
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ffmpeg \
    python3 \
    curl \
    ca-certificates \
    openssl \
    && rm -rf /var/lib/apt/lists/*

# Download latest standalone yt-dlp binary (yt-dlp_linux bundles curl-cffi for impersonation)
RUN curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux -o /usr/local/bin/yt-dlp && \
    chmod a+rx /usr/local/bin/yt-dlp

WORKDIR /app
COPY --from=builder /usr/src/app/target/release/kiwi-downloader /usr/local/bin/kiwi-downloader

# Environment variables
ENV YT_DLP_BIN=/usr/local/bin/yt-dlp
ENV DATABASE_URL=sqlite:///app/data/kiwi_cache.sqlite?mode=rwc
ENV DOWNLOAD_DIR=/app/data/downloads

# Create the data directory and set permissions for non-root execution (e.g., Koyeb, Hugging Face)
RUN mkdir -p /app/data/downloads && \
    chown -R 1000:1000 /app

# Switch to a non-root user for better security
USER 1000

CMD ["kiwi-downloader"]
