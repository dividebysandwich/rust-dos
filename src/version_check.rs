//! The check for a newer rust-dos at startup: rust-dos.com's version.json,
//! asked on a thread of its own. A newer version shows under the box above
//! the first DOS prompt and as a notification over the picture. The answer
//! is kept in rust-dos's directory, for the box of a start whose answer
//! doesn't come in time.

/// What the website says the latest release is: `{"version": "1.6.0",
/// "published": "..."}`.
pub const URL: &str = "https://rust-dos.com/version.json";
/// This rust-dos's version.
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Where to get it, for the messages.
pub const SITE: &str = "rust-dos.com";
/// The file in rust-dos's directory with the version last answered.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
const REMEMBERED: &str = "latest-version";

/// The version in version.json's text.
pub fn parse(text: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Latest {
        version: String,
    }
    let latest: Latest = serde_json::from_str(text).map_err(|e| format!("version.json: {}", e))?;
    let version = latest.version.trim().trim_start_matches('v').to_string();
    numbers(&version).ok_or_else(|| format!("version.json: '{}' is no version", latest.version))?;
    Ok(version)
}

/// The numbers of "1.6.0", without a "-beta" after them.
fn numbers(version: &str) -> Option<Vec<u64>> {
    let release = version.trim_start_matches('v').split(['-', '+']).next()?;
    release.split('.').map(|n| n.parse().ok()).collect()
}

/// Whether `latest` comes after `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (numbers(latest), numbers(current)) {
        (Some(mut latest), Some(mut current)) => {
            let len = latest.len().max(current.len());
            latest.resize(len, 0);
            current.resize(len, 0);
            latest > current
        }
        _ => false,
    }
}

/// The line under the box above the first prompt, and the notification's
/// two lines, of `version`.
pub fn console_line(version: &str) -> String {
    format!("Rust-DOS v{} is available (this is v{}): {}", version, CURRENT, SITE)
}

pub fn notification(version: &str) -> (String, String) {
    (
        format!("Rust-DOS v{} is available", version),
        format!("This is v{}. Get the new one at {}", CURRENT, SITE),
    )
}

/// The version an earlier check was answered.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
fn remembered() -> Option<String> {
    let text = std::fs::read_to_string(crate::config::user_dir()?.join(REMEMBERED)).ok()?;
    let version = text.trim().to_string();
    numbers(&version).map(|_| version)
}

#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
fn remember(version: &str) {
    if let Some(dir) = crate::config::user_dir()
        && std::fs::create_dir_all(&dir).is_ok()
    {
        let _ = std::fs::write(dir.join(REMEMBERED), format!("{}\n", version));
    }
}

/// A check under way, or answered.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
pub struct Check {
    pending: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// The version answered, until `news` takes it.
    answered: Option<String>,
    started: std::time::Instant,
}

#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
impl Check {
    /// Ask the website, on a thread of its own.
    pub fn start() -> Self {
        let (done, pending) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("version-check".to_string()).spawn(move || {
            let _ = done.send(fetch());
        });
        Self { pending: spawned.is_ok().then_some(pending), answered: None, started: std::time::Instant::now() }
    }

    fn receive(&mut self, result: Result<String, String>) {
        self.pending = None;
        match result {
            Ok(version) => {
                remember(&version);
                self.answered = Some(version);
            }
            Err(e) => eprintln!("[VERSION] The check for a new version failed: {}", e),
        }
    }

    /// Wait for the answer until `limit` after the start.
    pub fn wait(&mut self, limit: std::time::Duration) {
        use std::sync::mpsc::RecvTimeoutError;
        let Some(pending) = &self.pending else { return };
        match pending.recv_timeout(limit.saturating_sub(self.started.elapsed())) {
            Ok(result) => self.receive(result),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => self.pending = None,
        }
    }

    /// The newer version for the box above the first prompt: the one
    /// answered, or else the one an earlier check was.
    pub fn banner(&self) -> Option<String> {
        self.answered.clone().or_else(remembered).filter(|v| is_newer(v, CURRENT))
    }

    /// The newer version, once, when the answer has come.
    pub fn news(&mut self) -> Option<String> {
        use std::sync::mpsc::TryRecvError;
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(result) => self.receive(result),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        self.answered.take().filter(|v| is_newer(v, CURRENT))
    }

    /// Whether nothing more will come.
    pub fn finished(&self) -> bool {
        self.pending.is_none() && self.answered.is_none()
    }
}

/// Ask the website for version.json, past caches.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
fn fetch() -> Result<String, String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let url = format!("{}?p=rust-dos&t={}", URL, now.as_millis());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .user_agent(format!("rust-dos/{}", CURRENT))
        .build()
        .into();
    let text = agent
        .get(&url)
        .call()
        .and_then(|mut response| response.body_mut().with_config().limit(4096).read_to_string())
        .map_err(|e| format!("{} didn't answer: {}", SITE, e))?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_website_answers_a_version() {
        assert_eq!(parse(r#"{"version": "1.6.0", "published": "2026-10-10T18:59:50Z"}"#), Ok("1.6.0".to_string()));
        assert_eq!(parse(r#"{"version": "v1.7.2"}"#), Ok("1.7.2".to_string()));
        assert!(parse(r#"{"version": "soon"}"#).is_err());
        assert!(parse("<html>").is_err());
    }

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(is_newer("1.6.2", "1.6.1"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0", "1.99.99"));
        assert!(is_newer("1.6.0.1", "1.6"));
        assert!(!is_newer("1.6.1", "1.6.1"));
        assert!(!is_newer("1.6.0", "1.6.1"));
        assert!(!is_newer("1.6", "1.6.0"));
        assert!(!is_newer("1.7.0-beta", "1.7.0"));
        assert!(!is_newer("garbage", "1.6.1"));
        assert!(!is_newer("1.6.1", "garbage"));
        assert!(!is_newer("1.7.0", "1.7.0-beta"));
        assert!(!is_newer("1.6.9", "1.7.0"));
        assert!(!is_newer("0.9.99", "1.0"));
    }
}
