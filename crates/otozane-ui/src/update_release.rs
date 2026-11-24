//! Update checks and verified downloads from GitHub releases.
//! The functions block, so callers run them off the UI thread.

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "binido/Otozane";
pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/binido/Otozane/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const RELEASE_LIMIT: u64 = 2 << 20;
const CHECKSUM_LIMIT: u64 = 1 << 20;
const ASSET_LIMIT: u64 = 512 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// GitHub release metadata used by the update check.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// One downloadable file attached to a release.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// Release newer than the running build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub release: Release,
    pub version: Version,
}

/// HTTP response reduced to the fields the update check needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub rate_limited: bool,
    pub reset_at: Option<u64>,
    pub body: Vec<u8>,
}

/// Fetches URLs for the update check. Implemented by `HttpTransport` at runtime and by fixtures in tests.
pub trait Transport {
    /// Fetches a URL into memory. Non-success responses carry an empty body.
    fn get(&self, url: &str, limit: u64) -> Result<Response, UpdateError>;

    /// Streams a URL into `dest`. Writes nothing when the response is not successful.
    fn download(&self, url: &str, dest: &Path, limit: u64) -> Result<Response, UpdateError>;
}

/// HTTPS transport with timeouts and response size limits.
pub struct HttpTransport {
    client: reqwest::blocking::Client,
}

impl HttpTransport {
    /// Creates the transport. The client is reused across checks and downloads.
    pub fn new() -> Result<Self, UpdateError> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.url().scheme() == "https" {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .user_agent(format!(
                "otozane/{CURRENT_VERSION} (+https://github.com/{REPO})"
            ))
            .build()
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        Ok(Self { client })
    }

    fn request(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<reqwest::blocking::Response, UpdateError> {
        ensure_https(url)?;
        self.client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .timeout(timeout)
            .send()
            .map_err(|e| UpdateError::Network(e.to_string()))
    }
}

impl Transport for HttpTransport {
    fn get(&self, url: &str, limit: u64) -> Result<Response, UpdateError> {
        let resp = self.request(url, METADATA_TIMEOUT)?;
        let mut reply = response_head(&resp, true);
        if !is_success(reply.status) {
            return Ok(reply);
        }
        if resp.content_length().is_some_and(|n| n > limit) {
            return Err(UpdateError::SizeLimit {
                url: url.to_string(),
                limit,
            });
        }
        let mut body = Vec::new();
        resp.take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if body.len() as u64 > limit {
            return Err(UpdateError::SizeLimit {
                url: url.to_string(),
                limit,
            });
        }
        reply.body = body;
        Ok(reply)
    }

    fn download(&self, url: &str, dest: &Path, limit: u64) -> Result<Response, UpdateError> {
        let resp = self.request(url, DOWNLOAD_TIMEOUT)?;
        let reply = response_head(&resp, false);
        if !is_success(reply.status) {
            return Ok(reply);
        }
        if resp.content_length().is_some_and(|n| n > limit) {
            return Err(UpdateError::SizeLimit {
                url: url.to_string(),
                limit,
            });
        }
        let result = write_body(resp, dest, limit, url);
        if result.is_err() {
            let _ = fs::remove_file(dest);
        }
        result?;
        Ok(reply)
    }
}

/// Latest release from the GitHub API.
pub fn latest_release(transport: &dyn Transport) -> Result<Release, UpdateError> {
    let reply = transport.get(LATEST_RELEASE_URL, RELEASE_LIMIT)?;
    ensure_success(&reply)?;
    parse_release(&reply.body)
}

/// Parses the latest release response body.
pub fn parse_release(json: &[u8]) -> Result<Release, UpdateError> {
    serde_json::from_slice(json).map_err(|e| UpdateError::InvalidResponse(e.to_string()))
}

/// Checks whether a release tag is newer than `current`.
#[cfg(test)]
pub fn is_newer(current: &str, tag: &str) -> Result<bool, UpdateError> {
    Ok(parse_tag(tag)? > parse_tag(current)?)
}

/// Returns the latest release when its tag is newer than `current`.
pub fn check(
    transport: &dyn Transport,
    current: &str,
) -> Result<Option<AvailableUpdate>, UpdateError> {
    let release = latest_release(transport)?;
    let version = release.version()?;
    if version > parse_tag(current)? {
        Ok(Some(AvailableUpdate { release, version }))
    } else {
        Ok(None)
    }
}

/// Downloads an exact asset name into `staging` and verifies it against the same release's `SHA256SUMS`.
///
/// The returned path is final only after the checksum matches. A failed download leaves no file in
/// `staging` and never writes outside it.
pub fn download_verified(
    transport: &dyn Transport,
    release: &Release,
    asset_name: &str,
    staging: &Path,
) -> Result<PathBuf, UpdateError> {
    ensure_asset_name(asset_name)?;
    let asset = release.asset(asset_name)?;
    if asset.size > ASSET_LIMIT {
        return Err(UpdateError::SizeLimit {
            url: asset.browser_download_url.clone(),
            limit: ASSET_LIMIT,
        });
    }
    let sums_asset = release.asset("SHA256SUMS")?;
    ensure_https(&asset.browser_download_url)?;
    ensure_https(&sums_asset.browser_download_url)?;

    let sums_reply = transport.get(&sums_asset.browser_download_url, CHECKSUM_LIMIT)?;
    ensure_success(&sums_reply)?;
    let sums = std::str::from_utf8(&sums_reply.body)
        .map_err(|e| UpdateError::InvalidResponse(e.to_string()))?;
    let expected = checksum_for(sums, asset_name).ok_or_else(|| UpdateError::ChecksumMissing {
        name: asset_name.to_string(),
    })?;

    fs::create_dir_all(staging).map_err(UpdateError::Io)?;
    let path = staging.join(asset_name);
    // ponytail: one staging dir per update; two downloads of the same asset race on the part file.
    let part = staging.join(format!(".{asset_name}.part"));
    let _ = fs::remove_file(&part);

    let reply = match transport.download(&asset.browser_download_url, &part, ASSET_LIMIT) {
        Ok(reply) => reply,
        Err(e) => {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
    };
    if let Err(e) = ensure_success(&reply) {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    let actual = match sha256_file(&part) {
        Ok(hash) => hash,
        Err(e) => {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
    };
    if !actual.eq_ignore_ascii_case(&expected) {
        let _ = fs::remove_file(&part);
        return Err(UpdateError::ChecksumMismatch {
            name: asset_name.to_string(),
            expected,
            actual,
        });
    }
    fs::rename(&part, &path).map_err(|e| {
        let _ = fs::remove_file(&part);
        UpdateError::Io(e)
    })?;
    Ok(path)
}

impl Release {
    /// Parses `tag_name` as a version, accepting a leading `v`.
    pub fn version(&self) -> Result<Version, UpdateError> {
        parse_tag(&self.tag_name)
    }

    /// Returns the asset with exactly this name.
    pub fn asset(&self, name: &str) -> Result<&Asset, UpdateError> {
        ensure_asset_name(name)?;
        self.assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| UpdateError::AssetNotFound {
                name: name.to_string(),
            })
    }

    /// Release notes from the GitHub release body.
    pub fn notes(&self) -> &str {
        &self.body
    }
}

/// Update failures shown to the user.
#[derive(Debug)]
pub enum UpdateError {
    /// The request did not reach GitHub or the connection failed.
    Network(String),
    /// GitHub refused the request because the unauthenticated quota is exhausted.
    RateLimited { reset_at: Option<u64> },
    /// GitHub answered with a status other than success.
    Http { status: u16 },
    /// The release response is not valid GitHub release JSON.
    InvalidResponse(String),
    /// A release tag or the running version is not semantic.
    InvalidVersion { value: String, reason: String },
    /// The release has no asset with the requested exact name.
    AssetNotFound { name: String },
    /// `SHA256SUMS` has no entry for the requested asset.
    ChecksumMissing { name: String },
    /// The downloaded asset does not match `SHA256SUMS`.
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    /// The response exceeded the configured size limit.
    SizeLimit { url: String, limit: u64 },
    /// The URL is not HTTPS.
    InsecureUrl { url: String },
    /// The asset name is not a plain file name.
    InvalidAssetName { name: String },
    /// A local file operation failed.
    Io(std::io::Error),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(e) => write!(f, "could not reach GitHub: {e}"),
            UpdateError::RateLimited { reset_at: Some(t) } => {
                write!(f, "GitHub rate limit reached, retry after Unix time {t}")
            }
            UpdateError::RateLimited { reset_at: None } => {
                write!(f, "GitHub rate limit reached")
            }
            UpdateError::Http { status } => write!(f, "GitHub returned HTTP {status}"),
            UpdateError::InvalidResponse(e) => write!(f, "invalid GitHub response: {e}"),
            UpdateError::InvalidVersion { value, reason } => {
                write!(f, "invalid version {value}: {reason}")
            }
            UpdateError::AssetNotFound { name } => {
                write!(f, "the release has no asset named {name}")
            }
            UpdateError::ChecksumMissing { name } => {
                write!(f, "SHA256SUMS has no checksum for {name}")
            }
            UpdateError::ChecksumMismatch {
                name,
                expected,
                actual,
            } => write!(
                f,
                "checksum mismatch for {name}: expected {expected}, got {actual}"
            ),
            UpdateError::SizeLimit { url, limit } => {
                write!(f, "response from {url} exceeds {limit} bytes")
            }
            UpdateError::InsecureUrl { url } => write!(f, "refusing non-HTTPS URL {url}"),
            UpdateError::InvalidAssetName { name } => write!(f, "invalid asset name {name}"),
            UpdateError::Io(e) => write!(f, "file error: {e}"),
        }
    }
}

impl std::error::Error for UpdateError {}

fn parse_tag(tag: &str) -> Result<Version, UpdateError> {
    let value = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(value).map_err(|e| UpdateError::InvalidVersion {
        value: tag.to_string(),
        reason: e.to_string(),
    })
}

fn ensure_https(url: &str) -> Result<(), UpdateError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(UpdateError::InsecureUrl {
            url: url.to_string(),
        })
    }
}

fn ensure_asset_name(name: &str) -> Result<(), UpdateError> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\');
    if plain {
        Ok(())
    } else {
        Err(UpdateError::InvalidAssetName {
            name: name.to_string(),
        })
    }
}

fn is_success(status: u16) -> bool {
    (200..=299).contains(&status)
}

fn ensure_success(reply: &Response) -> Result<(), UpdateError> {
    if is_success(reply.status) {
        Ok(())
    } else if reply.rate_limited {
        Err(UpdateError::RateLimited {
            reset_at: reply.reset_at,
        })
    } else {
        Err(UpdateError::Http {
            status: reply.status,
        })
    }
}

fn response_head(resp: &reqwest::blocking::Response, api: bool) -> Response {
    let status = resp.status().as_u16();
    let remaining = resp
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok());
    let reset_at = resp
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    Response {
        status,
        // The GitHub API uses 403 for an exhausted quota, while asset CDN 403s mean something else.
        rate_limited: status == 429 || (api && status == 403) || remaining == Some("0"),
        reset_at,
        body: Vec::new(),
    }
}

fn write_body(
    mut reader: impl Read,
    dest: &Path,
    limit: u64,
    url: &str,
) -> Result<(), UpdateError> {
    let mut file = fs::File::create(dest).map_err(UpdateError::Io)?;
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > limit {
            return Err(UpdateError::SizeLimit {
                url: url.to_string(),
                limit,
            });
        }
        file.write_all(&buf[..n]).map_err(UpdateError::Io)?;
    }
    file.flush().map_err(UpdateError::Io)
}

fn sha256_file(path: &Path) -> Result<String, UpdateError> {
    let mut file = fs::File::open(path).map_err(UpdateError::Io)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(UpdateError::Io)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn checksum_for(sums: &str, name: &str) -> Option<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(file)) = (parts.next(), parts.next()) else {
            continue;
        };
        let file = file.strip_prefix('*').unwrap_or(file);
        if file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_JSON: &str = include_str!("../tests/fixtures/release_latest.json");
    const SUMS: &str = include_str!("../tests/fixtures/SHA256SUMS");
    const DMG: &[u8] = b"fake dmg payload\n";

    struct Fixture {
        release: Response,
        sums: Response,
        asset: Vec<u8>,
        download_error: bool,
    }

    impl Fixture {
        fn new(sums: &str) -> Self {
            Self {
                release: Response {
                    status: 200,
                    body: RELEASE_JSON.as_bytes().to_vec(),
                    ..Response::default()
                },
                sums: Response {
                    status: 200,
                    body: sums.as_bytes().to_vec(),
                    ..Response::default()
                },
                asset: DMG.to_vec(),
                download_error: false,
            }
        }
    }

    impl Transport for Fixture {
        fn get(&self, url: &str, _limit: u64) -> Result<Response, UpdateError> {
            if url == LATEST_RELEASE_URL {
                Ok(self.release.clone())
            } else if url.ends_with("/SHA256SUMS") {
                Ok(self.sums.clone())
            } else {
                Err(UpdateError::Http { status: 404 })
            }
        }

        fn download(&self, _url: &str, dest: &Path, _limit: u64) -> Result<Response, UpdateError> {
            if self.download_error {
                fs::write(dest, b"partial").unwrap();
                return Err(UpdateError::SizeLimit {
                    url: _url.to_string(),
                    limit: 1,
                });
            }
            fs::write(dest, &self.asset).unwrap();
            Ok(Response {
                status: 200,
                ..Response::default()
            })
        }
    }

    fn release() -> Release {
        parse_release(RELEASE_JSON.as_bytes()).unwrap()
    }

    #[test]
    fn release_fixture_parses_and_versions_order() {
        let release = release();
        assert_eq!(release.version().unwrap(), Version::new(0, 6, 0));
        assert!(release.notes().contains("Update from the app"));
        assert!(is_newer("0.5.0", "v0.6.0").unwrap());
        assert!(!is_newer("0.6.0", "v0.6.0").unwrap());
        assert!(is_newer("0.9.9", "v0.10.0").unwrap());
        assert!(!is_newer("0.10.0", "v0.9.9").unwrap());
        assert!(is_newer("0.6.0", "v0.6.1-beta.1").unwrap());
        assert!(matches!(
            is_newer("0.5.0", "nightly"),
            Err(UpdateError::InvalidVersion { .. })
        ));
    }

    #[test]
    fn asset_lookup_is_exact() {
        let release = release();
        assert!(release.asset("otozane-v0.6.0-macos-arm64.dmg").is_ok());
        assert!(matches!(
            release.asset("otozane-v0.6.0-macos-arm64"),
            Err(UpdateError::AssetNotFound { .. })
        ));
        assert!(matches!(
            release.asset("../otozane-v0.6.0-macos-arm64.dmg"),
            Err(UpdateError::InvalidAssetName { .. })
        ));
    }

    #[test]
    fn install_package_names_resolve_exactly() {
        let release = release();
        for name in [
            "otozane-v0.6.0-macos-arm64.dmg",
            "otozane_0.6.0-1_amd64.deb",
            "otozane_0.6.0-1_arm64.deb",
            "otozane-0.6.0-1.x86_64.rpm",
            "otozane-0.6.0-1.aarch64.rpm",
            "otozane-v0.6.0-linux-x86_64.tar.gz",
            "otozane-v0.6.0-linux-aarch64.tar.gz",
            "PKGBUILD",
        ] {
            assert!(release.asset(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn check_returns_newer_release_only() {
        let fixture = Fixture::new(SUMS);
        let update = check(&fixture, "0.5.0").unwrap().unwrap();
        assert_eq!(update.version, Version::new(0, 6, 0));
        assert!(check(&fixture, "0.6.0").unwrap().is_none());
        assert!(check(&fixture, "0.7.0").unwrap().is_none());
    }

    #[test]
    fn https_transport_builds() {
        HttpTransport::new().unwrap();
    }

    #[test]
    fn rate_limit_is_structured() {
        let mut fixture = Fixture::new(SUMS);
        fixture.release = Response {
            status: 403,
            rate_limited: true,
            reset_at: Some(1_800_000_000),
            body: Vec::new(),
        };
        assert!(matches!(
            latest_release(&fixture),
            Err(UpdateError::RateLimited {
                reset_at: Some(1_800_000_000)
            })
        ));
    }

    #[test]
    fn http_status_is_structured() {
        let mut fixture = Fixture::new(SUMS);
        fixture.release = Response {
            status: 500,
            ..Response::default()
        };
        assert!(matches!(
            latest_release(&fixture),
            Err(UpdateError::Http { status: 500 })
        ));
    }

    #[test]
    fn network_failure_is_structured() {
        struct Offline;
        impl Transport for Offline {
            fn get(&self, _: &str, _: u64) -> Result<Response, UpdateError> {
                Err(UpdateError::Network("offline".into()))
            }
            fn download(&self, _: &str, _: &Path, _: u64) -> Result<Response, UpdateError> {
                Err(UpdateError::Network("offline".into()))
            }
        }
        assert!(matches!(
            check(&Offline, "0.5.0"),
            Err(UpdateError::Network(_))
        ));
    }

    #[test]
    fn verified_download_stages_the_exact_asset() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(SUMS);
        let path = download_verified(
            &fixture,
            &release(),
            "otozane-v0.6.0-macos-arm64.dmg",
            dir.path(),
        )
        .unwrap();
        assert_eq!(path.file_name().unwrap(), "otozane-v0.6.0-macos-arm64.dmg");
        assert_eq!(fs::read(&path).unwrap(), DMG);
        assert!(
            !dir.path()
                .join(".otozane-v0.6.0-macos-arm64.dmg.part")
                .exists()
        );
    }

    #[test]
    fn checksum_mismatch_leaves_the_installation_untouched() {
        let install = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let marker = install.path().join("otozane");
        fs::write(&marker, b"installed").unwrap();
        let bad_sums = SUMS.replace(
            "4bfeab0aa1c4b426af048ffa79405a3bf2f3d71f4b534f50161ecc6b84585d20",
            &"0".repeat(64),
        );
        let fixture = Fixture::new(&bad_sums);
        let result = download_verified(
            &fixture,
            &release(),
            "otozane-v0.6.0-macos-arm64.dmg",
            staging.path(),
        );
        assert!(matches!(result, Err(UpdateError::ChecksumMismatch { .. })));
        assert_eq!(fs::read(&marker).unwrap(), b"installed");
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
    }

    #[test]
    fn missing_asset_and_missing_checksum_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(SUMS);
        assert!(matches!(
            download_verified(
                &fixture,
                &release(),
                "otozane-v0.6.0-linux-riscv64.tar.gz",
                dir.path()
            ),
            Err(UpdateError::AssetNotFound { .. })
        ));
        let no_entry = SUMS
            .lines()
            .filter(|l| !l.contains("macos-arm64.dmg"))
            .collect::<Vec<_>>()
            .join("\n");
        let fixture = Fixture::new(&no_entry);
        assert!(matches!(
            download_verified(
                &fixture,
                &release(),
                "otozane-v0.6.0-macos-arm64.dmg",
                dir.path()
            ),
            Err(UpdateError::ChecksumMissing { .. })
        ));
    }

    #[test]
    fn failed_download_removes_the_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new(SUMS);
        fixture.download_error = true;
        let result = download_verified(
            &fixture,
            &release(),
            "otozane-v0.6.0-macos-arm64.dmg",
            dir.path(),
        );
        assert!(matches!(result, Err(UpdateError::SizeLimit { .. })));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn rejects_non_https_asset_urls() {
        let mut release = release();
        let asset = release
            .assets
            .iter_mut()
            .find(|asset| asset.name.ends_with(".dmg"))
            .unwrap();
        asset.browser_download_url = "http://example.com/file".into();
        let name = asset.name.clone();
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(SUMS);
        let result = download_verified(&fixture, &release, &name, dir.path());
        assert!(matches!(result, Err(UpdateError::InsecureUrl { .. })));
    }
}
