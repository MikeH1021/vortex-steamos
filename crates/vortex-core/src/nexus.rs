//! Nexus Mods search and download.
//!
//! Name search uses the public GraphQL endpoint and does not need an API key.
//! File lists and downloads use the v1 API. The personal API key is read from
//! `VORTEX_NEXUS_KEY` or from a mode-0600 file. It is never printed.
//!
//! A Premium account can ask the API for a download link directly. A free
//! account gets one by pressing "Mod Manager Download" on the website, which
//! opens an `nxm://` link carrying a short-lived grant. [`NxmLink`] reads it.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

const API_ROOT: &str = "https://api.nexusmods.com";
const GRAPHQL_URL: &str = "https://api.nexusmods.com/v2/graphql";
const APP_NAME: &str = "vortex-steamos";
const USER_AGENT: &str = concat!("vortex-steamos/", env!("CARGO_PKG_VERSION"), " (Linux)");
const PAGE_SIZE: u32 = 20;
/// Stop a runaway response. The largest mod archives are a little over 10 GB.
const MAX_DOWNLOAD_BYTES: u64 = 32 * 1024 * 1024 * 1024;

const SEARCH_QUERY: &str = r#"
query SearchMods($filter: ModsFilter, $sort: [ModsSort!], $offset: Int, $count: Int) {
  mods(filter: $filter, sort: $sort, offset: $offset, count: $count) {
    totalCount
    nodes {
      modId
      name
      summary
      pictureUrl
      thumbnailUrl
      endorsements
      downloads
      version
      author
      adultContent
    }
  }
}
"#;

const PREMIUM_DOWNLOAD: &str = "Only Premium accounts can download from inside the app. With a free account, open the mod on the Nexus website and press Mod Manager Download. This app picks the download up from there.";
const EXPIRED_LINK: &str =
    "That download link has expired. Press Mod Manager Download on the Nexus page again.";
const RATE_LIMITED: &str = "Nexus is rate limiting this account. Wait a few minutes and try again.";
const NEED_KEY: &str =
    "Sign in with a Nexus API key to list and download files. Search works without one.";

#[derive(Debug)]
pub enum NexusError {
    Io(io::Error),
    Message(String),
}

impl std::fmt::Display for NexusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NexusError::Io(err) => write!(f, "{err}"),
            NexusError::Message(text) => write!(f, "{text}"),
        }
    }
}

impl std::error::Error for NexusError {}

impl From<io::Error> for NexusError {
    fn from(err: io::Error) -> Self {
        NexusError::Io(err)
    }
}

fn message(text: impl Into<String>) -> NexusError {
    NexusError::Message(text.into())
}

pub struct HttpText {
    pub status: u16,
    pub body: String,
}

/// Blocking Nexus client. Tests and the screenshot fixture supply their own.
pub trait NexusTransport {
    fn graphql(&self, body: &str, key: Option<&str>) -> Result<String, NexusError>;
    fn get(&self, path: &str, key: Option<&str>) -> Result<HttpText, NexusError>;
    /// Save `url` to `dest`. `progress` gets bytes received and the total when known.
    fn download_to(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), NexusError>;
}

pub struct LiveTransport {
    agent: ureq::Agent,
    /// No overall deadline. A large archive on hotel Wi-Fi takes as long as it
    /// takes, and only a connection that goes quiet is given up on.
    downloads: ureq::Agent,
}

impl LiveTransport {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .timeout_read(Duration::from_secs(60))
            .redirects(4)
            .https_only(true)
            .user_agent(USER_AGENT)
            .build();
        let downloads = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .redirects(4)
            .https_only(true)
            .user_agent(USER_AGENT)
            .build();
        Self { agent, downloads }
    }

    fn exchange(
        &self,
        url: &str,
        body: Option<&str>,
        key: Option<&str>,
    ) -> Result<HttpText, NexusError> {
        require_https(url)?;
        let mut request = if body.is_some() {
            self.agent.post(url)
        } else {
            self.agent.get(url)
        };
        request = request
            .set("Accept", "application/json")
            .set("Application-Name", APP_NAME);
        if body.is_some() {
            request = request.set("Content-Type", "application/json");
        }
        if let Some(key) = key {
            if url.starts_with("https://api.nexusmods.com/") {
                request = request.set("apikey", key);
            }
        }
        let result = if let Some(body) = body {
            request.send_string(body)
        } else {
            request.call()
        };
        match result {
            Ok(response) => Ok(HttpText {
                status: response.status(),
                body: response_text(response)?,
            }),
            Err(ureq::Error::Status(code, response)) => Ok(HttpText {
                status: code,
                body: response_text(response)?,
            }),
            Err(err) => Err(message(redact(&err.to_string(), key))),
        }
    }
}

impl Default for LiveTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl NexusTransport for LiveTransport {
    fn graphql(&self, body: &str, key: Option<&str>) -> Result<String, NexusError> {
        let response = self.exchange(GRAPHQL_URL, Some(body), key)?;
        if response.status >= 400 {
            return Err(message(format!(
                "Nexus search failed ({})",
                response.status
            )));
        }
        Ok(response.body)
    }

    fn get(&self, path: &str, key: Option<&str>) -> Result<HttpText, NexusError> {
        self.exchange(&format!("{API_ROOT}{path}"), None, key)
    }

    fn download_to(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), NexusError> {
        require_https(url)?;
        let response = match self.downloads.get(url).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(code, _)) => {
                return Err(message(format!("Download failed ({code}).")));
            }
            // The link carries a signed token. Keep it out of the message.
            Err(ureq::Error::Transport(err)) => {
                return Err(message(format!("Download failed: {}.", err.kind())));
            }
        };
        let total = response
            .header("Content-Length")
            .and_then(|value| value.trim().parse::<u64>().ok());
        write_capped(response.into_reader(), dest, total, progress)
    }
}

/// Reads JSON files from a directory so the shell can be captured without Nexus.
pub struct FixtureTransport {
    dir: PathBuf,
}

impl FixtureTransport {
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn read_text(&self, name: &str) -> Result<String, NexusError> {
        let path = self.dir.join(name);
        fs::read_to_string(&path).map_err(|err| message(format!("{}: {err}", path.display())))
    }

    fn status_file(&self, name: &str) -> u16 {
        fs::read_to_string(self.dir.join(name))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(200)
    }
}

impl NexusTransport for FixtureTransport {
    fn graphql(&self, _body: &str, _key: Option<&str>) -> Result<String, NexusError> {
        self.read_text("graphql.json")
    }

    fn get(&self, path: &str, _key: Option<&str>) -> Result<HttpText, NexusError> {
        if path.contains("download_link") {
            return Ok(HttpText {
                status: self.status_file("download_link.status"),
                body: self.read_text("download_link.json")?,
            });
        }
        if path.contains("files.json") {
            return Ok(HttpText {
                status: 200,
                body: self.read_text("files.json")?,
            });
        }
        if path.contains("/mods/") && !path.contains("/files") {
            return Ok(match self.read_text("mod.json") {
                Ok(body) => HttpText { status: 200, body },
                Err(_) => HttpText {
                    status: 404,
                    body: String::new(),
                },
            });
        }
        if path.contains("validate.json") {
            return Ok(HttpText {
                status: self.status_file("validate.status"),
                body: self.read_text("validate.json")?,
            });
        }
        Err(message(format!("No fixture response for {path}")))
    }

    fn download_to(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), NexusError> {
        require_https(url)?;
        let bytes = fs::read(self.dir.join("payload.bin"))?;
        let total = bytes.len() as u64;
        // A fixture can ask for a slow download so the progress bar can be seen.
        let delay = self.status_file("payload.delay_ms");
        if delay != 200 {
            for step in 1..=10 {
                progress(total * step / 10, Some(total));
                std::thread::sleep(Duration::from_millis(u64::from(delay) / 10));
            }
        }
        fs::write(dest, bytes)?;
        progress(total, Some(total));
        Ok(())
    }
}

fn require_https(url: &str) -> Result<(), NexusError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(message("Only https links are used."))
    }
}

fn response_text(response: ureq::Response) -> Result<String, NexusError> {
    response
        .into_string()
        .map_err(|err| message(format!("Nexus response was not text: {err}")))
}

fn write_capped(
    mut reader: impl Read,
    dest: &Path,
    expected: Option<u64>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), NexusError> {
    let mut file = fs::File::create(dest)?;
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut total = 0_u64;
    let result = (|| loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            if expected.is_some_and(|expected| total < expected) {
                return Err(message(
                    "The connection dropped before the download finished. Try again.",
                ));
            }
            file.flush()?;
            return Ok(());
        }
        total += read as u64;
        if total > MAX_DOWNLOAD_BYTES {
            return Err(message(
                "That download is larger than 32 GB. It was not saved.",
            ));
        }
        file.write_all(&buffer[..read])?;
        progress(total, expected);
    })();
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

fn redact(text: &str, key: Option<&str>) -> String {
    let Some(key) = key else {
        return text.to_string();
    };
    if key.len() < 4 || !text.contains(key) {
        return text.to_string();
    }
    text.replace(key, "[redacted]")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Account {
    pub signed_in: bool,
    pub name: String,
    pub premium: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ClearedKey {
    pub cleared: bool,
    pub env_override: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NexusMod {
    pub mod_id: u64,
    pub name: String,
    pub author: String,
    pub summary: String,
    pub version: String,
    pub endorsements: u64,
    pub downloads: u64,
    pub thumbnail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchReport {
    pub query: String,
    pub domain: String,
    pub mods: Vec<NexusMod>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NexusFile {
    pub file_id: u64,
    pub name: String,
    pub file_name: String,
    pub version: String,
    pub category: String,
    pub size_kb: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileList {
    pub mod_id: u64,
    pub files: Vec<NexusFile>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SavedDownload {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    pub name: String,
    pub mod_name: String,
    pub path: PathBuf,
}

/// What an `nxm://` link asks for.
///
/// `nxm://skyrimspecialedition/mods/667/files/1234?key=…&expires=…&user_id=…`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NxmLink {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    /// Present when a free account pressed the download button on the website.
    pub grant: Option<NxmGrant>,
}

/// The short-lived permission the website attaches to a download link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NxmGrant {
    pub key: String,
    pub expires: u64,
}

impl NxmLink {
    /// A file picked inside the app, with no grant from the website.
    pub fn direct(domain: &str, mod_id: u64, file_id: u64) -> Self {
        Self {
            domain: domain.to_string(),
            mod_id,
            file_id,
            grant: None,
        }
    }

    pub fn parse(link: &str) -> Result<Self, NexusError> {
        let bad = || message("That is not a Nexus download link.");
        let link = link.trim();
        let scheme = link.get(..6).ok_or_else(bad)?;
        if !scheme.eq_ignore_ascii_case("nxm://") {
            return Err(bad());
        }
        let rest = link[6..].split('#').next().unwrap_or("");
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let domain = parts.first().ok_or_else(bad)?.to_ascii_lowercase();
        if parts
            .get(1)
            .is_some_and(|part| part.eq_ignore_ascii_case("collections"))
        {
            return Err(message(
                "Collections are not supported yet. Download the mods one at a time.",
            ));
        }
        if domain == "oauth" || domain == "premium" {
            return Err(message("That Nexus link is not a download."));
        }
        let shape_ok = parts.len() == 5
            && parts[1].eq_ignore_ascii_case("mods")
            && parts[3].eq_ignore_ascii_case("files");
        if !shape_ok {
            return Err(bad());
        }
        domain_segment(&domain)?;
        let mod_id = parts[2].parse::<u64>().map_err(|_| bad())?;
        let file_id = parts[4].parse::<u64>().map_err(|_| bad())?;
        if mod_id == 0 || file_id == 0 {
            return Err(bad());
        }
        let mut key = None;
        let mut expires = None;
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            match name {
                "key" => key = Some(value),
                "expires" => expires = value.parse::<u64>().ok(),
                _ => {}
            }
        }
        // The grant is echoed into a request path. Only accept what Nexus
        // issues: URL-safe base64 and a Unix time.
        let grant = match (key, expires) {
            (Some(key), Some(expires))
                if !key.is_empty()
                    && key.len() <= 200
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
            {
                Some(NxmGrant {
                    key: key.to_string(),
                    expires,
                })
            }
            (None, _) => None,
            _ => return Err(bad()),
        };
        Ok(Self {
            domain,
            mod_id,
            file_id,
            grant,
        })
    }
}

pub fn default_key_path() -> PathBuf {
    config_dir().join("nexus.key")
}

fn config_dir() -> PathBuf {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("vortex-steamos");
        }
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("vortex-steamos");
    }
    PathBuf::from(".config").join("vortex-steamos")
}

pub fn current_key() -> Result<Option<String>, NexusError> {
    if let Ok(from_env) = env::var("VORTEX_NEXUS_KEY") {
        let trimmed = from_env.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed));
        }
    }
    read_key_file(&default_key_path())
}

pub fn read_key_file(path: &Path) -> Result<Option<String>, NexusError> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn store_key(source: &Path, transport: &dyn NexusTransport) -> Result<Account, NexusError> {
    store_key_at(source, &default_key_path(), transport)
}

pub fn store_key_at(
    source: &Path,
    dest: &Path,
    transport: &dyn NexusTransport,
) -> Result<Account, NexusError> {
    let meta = fs::metadata(source)?;
    if meta.len() > 4096 {
        return Err(message("That key file is too large."));
    }
    let raw = fs::read_to_string(source)?;
    let key = normalize_key(&raw)?;
    let account = validate_account(transport, &key)?;
    if let Some(parent) = dest.parent() {
        create_private_dir(parent)?;
    }
    write_secret(dest, &key)?;
    if source != dest {
        let _ = fs::remove_file(source);
    }
    Ok(account)
}

pub fn clear_key() -> Result<ClearedKey, NexusError> {
    clear_key_at(&default_key_path())
}

pub fn clear_key_at(path: &Path) -> Result<ClearedKey, NexusError> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let env_override = env::var("VORTEX_NEXUS_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    Ok(ClearedKey {
        cleared: true,
        env_override,
    })
}

pub fn validate_account(transport: &dyn NexusTransport, key: &str) -> Result<Account, NexusError> {
    let response = transport.get("/v1/users/validate.json", Some(key))?;
    if response.status == 401 || response.status == 403 {
        return Err(message("Nexus rejected that API key."));
    }
    if response.status >= 400 {
        let detail = api_message(&response.body, Some(key));
        if detail.is_empty() {
            return Err(message(format!(
                "Nexus could not check that key ({}).",
                response.status
            )));
        }
        return Err(message(format!(
            "Nexus could not check that key ({}): {detail}",
            response.status
        )));
    }
    parse_account(&response.body)
}

pub fn search_mods(
    transport: &dyn NexusTransport,
    domain: &str,
    query: &str,
    key: Option<&str>,
) -> Result<SearchReport, NexusError> {
    let trimmed: String = query.trim().chars().take(80).collect();
    let request = search_request(domain, &trimmed);
    let body = serde_json::to_string(&request).map_err(|err| message(err.to_string()))?;
    let response = transport.graphql(&body, key)?;
    parse_search(&response, domain, &trimmed)
}

pub fn list_mod_files(
    transport: &dyn NexusTransport,
    domain: &str,
    mod_id: u64,
    key: &str,
) -> Result<FileList, NexusError> {
    if key.is_empty() {
        return Err(message(NEED_KEY));
    }
    let path = format!(
        "/v1/games/{}/mods/{mod_id}/files.json",
        domain_segment(domain)?
    );
    let response = transport.get(&path, Some(key))?;
    if response.status == 401 || response.status == 403 {
        return Err(message("Nexus rejected that API key."));
    }
    if response.status == 404 {
        return Err(message("Nexus could not find that mod."));
    }
    if response.status == 429 {
        return Err(message(RATE_LIMITED));
    }
    if response.status >= 400 {
        return Err(message(format!(
            "Nexus could not list files ({}).",
            response.status
        )));
    }
    parse_files(mod_id, &response.body)
}

/// Download the file `link` names into `incoming`.
///
/// The grant comes from an `nxm://` link. Without one Nexus only hands a
/// download address to Premium accounts.
pub fn download_mod_file(
    transport: &dyn NexusTransport,
    link: &NxmLink,
    incoming: &Path,
    key: &str,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<SavedDownload, NexusError> {
    if key.is_empty() {
        return Err(message(NEED_KEY));
    }
    let (mod_id, file_id, grant) = (link.mod_id, link.file_id, link.grant.as_ref());
    let domain = domain_segment(&link.domain)?;
    let files = list_mod_files(transport, domain, mod_id, key)?;
    let file = files
        .files
        .iter()
        .find(|file| file.file_id == file_id)
        .ok_or_else(|| message("That file is not on this mod."))?;
    let file_name = archive_file_name(&file.file_name)?;
    let mut path = format!("/v1/games/{domain}/mods/{mod_id}/files/{file_id}/download_link.json");
    if let Some(grant) = grant {
        path.push_str(&format!("?key={}&expires={}", grant.key, grant.expires));
    }
    let response = transport.get(&path, Some(key))?;
    match response.status {
        401 => return Err(message("Nexus rejected that API key.")),
        403 if grant.is_none() => return Err(message(PREMIUM_DOWNLOAD)),
        400 | 403 | 410 if grant.is_some() => return Err(message(EXPIRED_LINK)),
        429 => return Err(message(RATE_LIMITED)),
        status if status >= 400 => {
            return Err(message(format!(
                "Nexus could not start that download ({status})."
            )));
        }
        _ => {}
    }
    let url = first_https_link(&response.body)?;
    let (mod_name, mod_version) = mod_summary(transport, domain, mod_id, key);
    fs::create_dir_all(incoming)?;
    let dest = vacant_archive(incoming, &file_name);
    if dest.parent() != Some(incoming) {
        return Err(message("Refusing to save a download outside incoming."));
    }
    let saved_name = dest
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or(file_name);
    let partial = incoming.join(format!(".{saved_name}.partial"));
    let downloaded = transport.download_to(&url, &partial, progress);
    if let Err(err) = downloaded {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    if let Err(err) = fs::rename(&partial, &dest) {
        let _ = fs::remove_file(&partial);
        return Err(err.into());
    }
    let meta = crate::install::DownloadMeta {
        domain: domain.to_string(),
        mod_id,
        file_id,
        mod_name: mod_name.clone(),
        file_title: file.name.clone(),
        version: if file.version.is_empty() {
            mod_version
        } else {
            file.version.clone()
        },
    };
    // The archive installs fine without this. It only loses its Nexus link.
    let _ = crate::install::write_meta(&dest, &meta);
    Ok(SavedDownload {
        domain: domain.to_string(),
        mod_id,
        file_id,
        name: saved_name,
        mod_name,
        path: dest,
    })
}

/// The mod's name and version, or empty strings when Nexus will not say.
fn mod_summary(
    transport: &dyn NexusTransport,
    domain: &str,
    mod_id: u64,
    key: &str,
) -> (String, String) {
    let path = format!("/v1/games/{domain}/mods/{mod_id}.json");
    let Ok(response) = transport.get(&path, Some(key)) else {
        return Default::default();
    };
    if response.status >= 400 {
        return Default::default();
    }
    let Ok(value) = serde_json::from_str::<Value>(&response.body) else {
        return Default::default();
    };
    let text = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .chars()
            .take(200)
            .collect::<String>()
    };
    (text("name"), text("version"))
}

pub(crate) fn search_request(domain: &str, query: &str) -> Value {
    let query = query.trim().chars().take(80).collect::<String>();
    let mut filter = json!({
        "gameDomainName": [{ "value": domain, "op": "EQUALS" }]
    });
    let sort_field = if query.is_empty() {
        "endorsements"
    } else {
        filter["nameStemmed"] = json!([{ "value": query, "op": "MATCHES" }]);
        "downloads"
    };
    json!({
        "query": SEARCH_QUERY,
        "variables": {
            "filter": filter,
            "sort": [{ sort_field: { "direction": "DESC" } }],
            "offset": 0,
            "count": PAGE_SIZE
        }
    })
}

fn parse_search(body: &str, domain: &str, query: &str) -> Result<SearchReport, NexusError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| message("Nexus search returned something that was not JSON."))?;
    let nodes = value.pointer("/data/mods/nodes").and_then(Value::as_array);
    let Some(nodes) = nodes else {
        if let Some(text) = graphql_error(&value) {
            return Err(message(text));
        }
        return Err(message("Nexus search returned no mod list."));
    };
    let mut mods = Vec::new();
    for node in nodes {
        if node.get("adultContent").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let mod_id = json_u64(node.get("modId").unwrap_or(&Value::Null));
        let name = node
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if mod_id == 0 || name.is_empty() {
            continue;
        }
        mods.push(NexusMod {
            mod_id,
            name: name.to_string(),
            author: node
                .get("author")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            summary: clip_text(node.get("summary").and_then(Value::as_str).unwrap_or("")),
            version: node
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            endorsements: json_u64(node.get("endorsements").unwrap_or(&Value::Null)),
            downloads: json_u64(node.get("downloads").unwrap_or(&Value::Null)),
            thumbnail: https_url(
                node.get("thumbnailUrl")
                    .and_then(Value::as_str)
                    .filter(|url| !url.is_empty())
                    .or_else(|| node.get("pictureUrl").and_then(Value::as_str))
                    .unwrap_or(""),
            ),
        });
        if mods.len() == PAGE_SIZE as usize {
            break;
        }
    }
    Ok(SearchReport {
        query: query.to_string(),
        domain: domain.to_string(),
        mods,
    })
}

fn parse_account(body: &str) -> Result<Account, NexusError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| message("Nexus did not return an account."))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() && value.get("user_id").is_none() {
        return Err(message("Nexus did not return an account."));
    }
    Ok(Account {
        signed_in: true,
        name,
        premium: value
            .get("is_premium")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_files(mod_id: u64, body: &str) -> Result<FileList, NexusError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| message("Nexus did not return a file list."))?;
    let Some(files) = value.get("files").and_then(Value::as_array) else {
        return Err(message("Nexus did not return a file list."));
    };
    let mut parsed = Vec::new();
    for file in files {
        let file_id = json_u64(file.get("file_id").unwrap_or(&Value::Null));
        if file_id == 0 {
            continue;
        }
        let category = file
            .get("category_name")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                category_label(json_u64(file.get("category_id").unwrap_or(&Value::Null)))
            });
        parsed.push(NexusFile {
            file_id,
            name: file
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("File")
                .trim()
                .to_string(),
            file_name: file
                .get("file_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            version: file
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            category,
            size_kb: json_u64(
                file.get("size_kb")
                    .or_else(|| file.get("size"))
                    .unwrap_or(&Value::Null),
            ),
        });
    }
    parsed.sort_by(|left, right| {
        category_rank(&left.category)
            .cmp(&category_rank(&right.category))
            .then(left.name.cmp(&right.name))
    });
    Ok(FileList {
        mod_id,
        files: parsed,
    })
}

fn first_https_link(body: &str) -> Result<String, NexusError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| message("Nexus did not return a download link."))?;
    let items: Vec<&Value> = if let Some(list) = value.as_array() {
        list.iter().collect()
    } else {
        vec![&value]
    };
    for item in items {
        for key in ["URI", "uri", "url"] {
            if let Some(url) = item.get(key).and_then(Value::as_str) {
                if url.starts_with("https://") {
                    return Ok(url.to_string());
                }
            }
        }
    }
    Err(message("Nexus did not return an https download link."))
}

/// Turn the file name Nexus reports into one that is safe to save.
///
/// Anything that could act as a path is refused. Characters a shell or a
/// Windows tool would trip over are replaced, since mod authors use plenty.
pub(crate) fn archive_file_name(raw: &str) -> Result<String, NexusError> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.contains('/')
        || raw.contains('\\')
        || raw.contains(':')
        || raw.contains("..")
        || raw.starts_with('.')
    {
        return Err(message(
            "Nexus returned a file name that is not a single archive.",
        ));
    }
    let lower = raw.to_ascii_lowercase();
    if !crate::install::ARCHIVE_EXTENSIONS
        .iter()
        .any(|extension| lower.ends_with(extension))
    {
        return Err(message(
            "That file is not a .zip, .7z, or .rar archive, so it cannot be installed as a mod. Download it from the website if you need it.",
        ));
    }
    let cleaned: String = raw
        .chars()
        .take(180)
        .map(|c| {
            let keep = c.is_alphanumeric()
                || matches!(
                    c,
                    '.' | '-' | '_' | ' ' | '(' | ')' | '+' | '[' | ']' | ',' | '\'' | '&' | '!'
                );
            if keep {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(cleaned)
}

fn vacant_archive(dir: &Path, name: &str) -> PathBuf {
    let direct = dir.join(name);
    if !direct.exists() {
        return direct;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("archive");
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("zip");
    for index in 2..1000 {
        let candidate = dir.join(format!("{stem}-{index}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-extra.{ext}"))
}

fn domain_segment(domain: &str) -> Result<&str, NexusError> {
    if domain.chars().all(|c| c.is_ascii_alphanumeric()) && !domain.is_empty() {
        Ok(domain)
    } else {
        Err(message("Unknown Nexus game."))
    }
}

fn normalize_key(raw: &str) -> Result<String, NexusError> {
    let key = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        return Err(message(
            "The key file should contain only the Nexus API key.",
        ));
    }
    if key.len() < 8
        || key.len() > 200
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(message("That file does not look like a Nexus API key."));
    }
    Ok(key.to_string())
}

fn create_private_dir(path: &Path) -> Result<(), NexusError> {
    if path.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

fn write_secret(path: &Path, key: &str) -> Result<(), NexusError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(key.as_bytes())?;
    file.write_all(b"\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn api_message(body: &str, key: Option<&str>) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    let text = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    redact(text, key)
}

fn graphql_error(value: &Value) -> Option<String> {
    let text = value
        .pointer("/errors/0/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn json_u64(value: &Value) -> u64 {
    match value {
        Value::Number(number) => number.as_u64().unwrap_or(0),
        Value::String(text) => text.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn https_url(url: &str) -> String {
    if url.starts_with("https://") {
        url.to_string()
    } else {
        String::new()
    }
}

fn clip_text(text: &str) -> String {
    let trimmed = text.trim();
    let mut clipped: String = trimmed.chars().take(500).collect();
    if trimmed.chars().count() > 500 {
        clipped.push('…');
    }
    clipped
}

fn category_label(id: u64) -> String {
    match id {
        1 => "MAIN",
        2 => "UPDATE",
        3 => "OPTIONAL",
        4 => "OLD_VERSION",
        5 => "MISCELLANEOUS",
        6 => "DELETED",
        _ => "FILE",
    }
    .to_string()
}

fn category_rank(category: &str) -> u8 {
    match category.to_ascii_uppercase().as_str() {
        "MAIN" => 0,
        "UPDATE" => 1,
        "OPTIONAL" => 2,
        "MISCELLANEOUS" | "MISC" => 3,
        "OLD_VERSION" => 4,
        "DELETED" => 6,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct Scripted {
        graphql: String,
        gets: Vec<(&'static str, u16, String)>,
        payload: Vec<u8>,
        downloads: Cell<u32>,
        paths: std::cell::RefCell<Vec<String>>,
    }

    impl NexusTransport for Scripted {
        fn graphql(&self, _body: &str, _key: Option<&str>) -> Result<String, NexusError> {
            Ok(self.graphql.clone())
        }

        fn get(&self, path: &str, _key: Option<&str>) -> Result<HttpText, NexusError> {
            self.paths.borrow_mut().push(path.to_string());
            for (needle, status, body) in &self.gets {
                if path.contains(needle) {
                    return Ok(HttpText {
                        status: *status,
                        body: body.clone(),
                    });
                }
            }
            Err(message(format!("no scripted response for {path}")))
        }

        fn download_to(
            &self,
            url: &str,
            dest: &Path,
            progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<(), NexusError> {
            require_https(url)?;
            self.downloads.set(self.downloads.get() + 1);
            fs::write(dest, &self.payload)?;
            progress(self.payload.len() as u64, Some(self.payload.len() as u64));
            Ok(())
        }
    }

    #[test]
    fn named_query_filters_by_name_and_sorts_by_downloads() {
        let body = search_request("skyrimspecialedition", "  campfire  ");
        let filter = &body["variables"]["filter"];
        assert_eq!(filter["gameDomainName"][0]["value"], "skyrimspecialedition");
        assert_eq!(filter["gameDomainName"][0]["op"], "EQUALS");
        assert_eq!(filter["nameStemmed"][0]["value"], "campfire");
        assert_eq!(filter["nameStemmed"][0]["op"], "MATCHES");
        assert!(body["variables"]["sort"][0].get("downloads").is_some());
        assert_eq!(body["variables"]["count"], 20);
    }

    #[test]
    fn empty_query_browses_by_endorsements() {
        let body = search_request("skyrim", "   ");
        assert!(body["variables"]["filter"].get("nameStemmed").is_none());
        assert!(body["variables"]["sort"][0].get("endorsements").is_some());
    }

    #[test]
    fn search_drops_adult_mods() {
        let graphql = r#"{
            "data": {
                "mods": {
                    "nodes": [
                        {
                            "modId": 10,
                            "name": "Campfire",
                            "summary": "Camping.",
                            "author": "Chesko",
                            "version": "1.12",
                            "endorsements": 3,
                            "downloads": "9",
                            "adultContent": false,
                            "thumbnailUrl": "https://cdn.example/camp.jpg"
                        },
                        {
                            "modId": 11,
                            "name": "Hidden",
                            "summary": "no",
                            "adultContent": true
                        }
                    ]
                }
            }
        }"#;
        let transport = Scripted {
            graphql: graphql.to_string(),
            gets: Vec::new(),
            payload: Vec::new(),
            downloads: Cell::new(0),
            paths: Default::default(),
        };
        let report = search_mods(&transport, "skyrimspecialedition", "campfire", None).unwrap();
        assert_eq!(report.mods.len(), 1);
        assert_eq!(report.mods[0].name, "Campfire");
        assert_eq!(report.mods[0].downloads, 9);
        assert_eq!(report.mods[0].thumbnail, "https://cdn.example/camp.jpg");
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("Hidden"));
    }

    #[test]
    fn files_parse_and_sort_main_first() {
        let body = r#"{
            "files": [
                {"file_id": 2, "name": "Old", "file_name": "old.zip", "version": "0.1", "category_id": 4, "size_kb": 10},
                {"file_id": 1, "name": "Campfire", "file_name": "Campfire.zip", "version": "1.12", "category_name": "MAIN", "size_kb": 20}
            ]
        }"#;
        let list = parse_files(55, body).unwrap();
        assert_eq!(list.files[0].file_id, 1);
        assert_eq!(list.files[0].category, "MAIN");
        assert_eq!(list.files[1].category, "OLD_VERSION");
    }

    #[test]
    fn rejects_path_like_file_names() {
        assert!(archive_file_name("../Campfire.zip").is_err());
        assert!(archive_file_name("mods/Campfire.zip").is_err());
        assert!(archive_file_name("C:\\Campfire.zip").is_err());
        assert!(archive_file_name("Campfire.exe").is_err());
        assert!(archive_file_name(".hidden.zip").is_err());
        assert!(archive_file_name("").is_err());
        assert_eq!(archive_file_name("Campfire.rar").unwrap(), "Campfire.rar");
        assert_eq!(
            archive_file_name("Bob's $5 `mod` & more.zip").unwrap(),
            "Bob's _5 _mod_ & more.zip"
        );
        assert_eq!(
            archive_file_name(" Campfire (1).7z ").unwrap(),
            "Campfire (1).7z"
        );
    }

    #[test]
    fn forbidden_download_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let incoming = dir.path().join("incoming");
        fs::create_dir_all(&incoming).unwrap();
        let transport = scripted_files(403, r#"{"message":"nope SECRETKEY"}"#);
        let err = download_mod_file(
            &transport,
            &NxmLink::direct("skyrimspecialedition", 55, 1),
            &incoming,
            "SECRETKEY",
            &mut |_, _| {},
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("Premium"));
        assert!(!text.contains("SECRETKEY"));
        assert_eq!(transport.downloads.get(), 0);
        assert!(fs::read_dir(&incoming).unwrap().next().is_none());
    }

    #[test]
    fn download_writes_the_archive_into_incoming() {
        let dir = tempfile::tempdir().unwrap();
        let incoming = dir.path().join("incoming");
        let transport = scripted_files(
            200,
            r#"[{"name":"Nexus CDN","short_name":"CDN","URI":"https://cdn.example/campfire.zip"}]"#,
        );
        let saved = download_mod_file(
            &transport,
            &NxmLink::direct("skyrimspecialedition", 55, 1),
            &incoming,
            "SECRETKEY",
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(saved.name, "Campfire.zip");
        assert_eq!(fs::read(&saved.path).unwrap(), b"PK fake");
        assert!(saved.path.starts_with(&incoming));
        assert_eq!(transport.downloads.get(), 1);
        let json = serde_json::to_string(&saved).unwrap();
        assert!(!json.contains("SECRETKEY"));
    }

    #[test]
    fn nxm_links_parse() {
        let link = NxmLink::parse(
            "nxm://SkyrimSpecialEdition/mods/667/files/1234?key=aB3-_x&expires=1700000000&user_id=5",
        )
        .unwrap();
        assert_eq!(link.domain, "skyrimspecialedition");
        assert_eq!((link.mod_id, link.file_id), (667, 1234));
        let grant = link.grant.unwrap();
        assert_eq!(grant.key, "aB3-_x");
        assert_eq!(grant.expires, 1_700_000_000);

        let premium = NxmLink::parse("nxm://fallout4/mods/1/files/2").unwrap();
        assert!(premium.grant.is_none());

        for bad in [
            "https://www.nexusmods.com/skyrim/mods/1",
            "nxm://skyrim/mods/1",
            "nxm://skyrim/mods/x/files/2",
            "nxm://skyrim/mods/0/files/2",
            "nxm://sky rim/mods/1/files/2",
            "nxm://skyrim/mods/1/files/2?key=a/../b&expires=1",
            "nxm://skyrim/mods/1/files/2?key=abc&expires=soon",
            "nxm://oauth/callback?code=1",
            "nxm",
        ] {
            assert!(NxmLink::parse(bad).is_err(), "{bad}");
        }
        let collection = NxmLink::parse("nxm://skyrim/collections/abc/revisions/3").unwrap_err();
        assert!(collection.to_string().contains("Collections"));
    }

    #[test]
    fn a_website_grant_is_sent_and_metadata_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let incoming = dir.path().join("incoming");
        let mut transport = scripted_files(200, r#"[{"URI":"https://cdn.example/campfire.zip"}]"#);
        transport.gets.push((
            "mods/55.json",
            200,
            r#"{"name":"Campfire - Complete Camping System","version":"1.12"}"#.to_string(),
        ));
        let link =
            NxmLink::parse("nxm://skyrimspecialedition/mods/55/files/1?key=GRANT&expires=99")
                .unwrap();
        let mut seen = 0;
        let saved = download_mod_file(
            &transport,
            &link,
            &incoming,
            "SECRETKEY",
            &mut |received, _| seen = received,
        )
        .unwrap();
        assert_eq!(seen, 7);
        assert_eq!(saved.mod_name, "Campfire - Complete Camping System");
        assert!(transport
            .paths
            .borrow()
            .iter()
            .any(|path| path.ends_with("download_link.json?key=GRANT&expires=99")));
        let meta = fs::read_to_string(crate::install::meta_path(&saved.path)).unwrap();
        assert!(meta.contains("\"mod_id\": 55"), "{meta}");
        assert!(meta.contains("Complete Camping"));
        assert!(!meta.contains("SECRETKEY") && !meta.contains("GRANT"));
        // Nothing half-written is left next to it.
        let names: Vec<String> = fs::read_dir(&incoming)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names.iter().all(|name| !name.ends_with(".partial")),
            "{names:?}"
        );
    }

    #[test]
    fn an_expired_grant_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let transport = scripted_files(410, "{}");
        let mut link = NxmLink::direct("skyrim", 55, 1);
        link.grant = Some(NxmGrant {
            key: "GRANT".into(),
            expires: 1,
        });
        let err = download_mod_file(
            &transport,
            &link,
            &dir.path().join("incoming"),
            "SECRETKEY",
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("expired"), "{err}");
    }

    #[test]
    fn a_short_download_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("half.zip");
        let err = write_capped(&b"abc"[..], &dest, Some(10), &mut |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("dropped"), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn http_link_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let transport = scripted_files(200, r#"[{"URI":"http://cdn.example/campfire.zip"}]"#);
        let err = download_mod_file(
            &transport,
            &NxmLink::direct("skyrim", 55, 1),
            &dir.path().join("incoming"),
            "SECRETKEY",
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("https"));
        assert_eq!(transport.downloads.get(), 0);
    }

    #[test]
    fn store_key_is_private_and_absent_from_the_account() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("pasted.key");
        let dest = dir.path().join("nexus.key");
        fs::write(&source, "abcdEFGH1234\n").unwrap();
        let transport = Scripted {
            graphql: String::new(),
            gets: vec![(
                "validate.json",
                200,
                r#"{"user_id":7,"key":"abcdEFGH1234","email":"ada@example.com","name":"Ada","is_premium":false}"#
                    .to_string(),
            )],
            payload: Vec::new(),
            downloads: Cell::new(0),
            paths: Default::default(),
        };
        let account = store_key_at(&source, &dest, &transport).unwrap();
        assert_eq!(account.name, "Ada");
        assert!(!account.premium);
        assert!(account.signed_in);
        let json = serde_json::to_string(&account).unwrap();
        assert!(!json.contains("abcdEFGH1234"));
        assert!(!json.contains("ada@example.com"));
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap().trim(), "abcdEFGH1234");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn rejected_key_is_not_echoed() {
        let transport = Scripted {
            graphql: String::new(),
            gets: vec![(
                "validate.json",
                401,
                r#"{"message":"bad SECRETKEYVALUE"}"#.to_string(),
            )],
            payload: Vec::new(),
            downloads: Cell::new(0),
            paths: Default::default(),
        };
        let err = validate_account(&transport, "SECRETKEYVALUE").unwrap_err();
        assert!(!err.to_string().contains("SECRETKEYVALUE"));
    }

    #[test]
    fn env_key_overrides_the_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nexus.key");
        fs::write(&path, "fromfile1\n").unwrap();
        let previous = env::var("VORTEX_NEXUS_KEY").ok();
        // SAFETY: this test holds ENV_LOCK so the harness does not run another
        // key-env test at the same time. The previous value is restored.
        unsafe {
            env::set_var("VORTEX_NEXUS_KEY", "fromenv22");
        }
        let key = current_key().unwrap();
        unsafe {
            match &previous {
                Some(value) => env::set_var("VORTEX_NEXUS_KEY", value),
                None => env::remove_var("VORTEX_NEXUS_KEY"),
            }
        }
        assert_eq!(key.as_deref(), Some("fromenv22"));
        let cleared = clear_key_at(&path).unwrap();
        assert!(cleared.cleared);
        assert!(!cleared.env_override);
        assert!(read_key_file(&path).unwrap().is_none());
    }

    fn scripted_files(link_status: u16, link_body: &str) -> Scripted {
        Scripted {
            graphql: String::new(),
            gets: vec![
                (
                    "files.json",
                    200,
                    r#"{"files":[{"file_id":1,"name":"Campfire","file_name":"Campfire.zip","version":"1.12","category_name":"MAIN","size_kb":4}]}"#
                        .to_string(),
                ),
                ("download_link", link_status, link_body.to_string()),
            ],
            payload: b"PK fake".to_vec(),
            downloads: Cell::new(0),
            paths: Default::default(),
        }
    }
}
