//! Checking for a newer Notary release. It contacts GitHub only when the user asks.

use anyhow::{Context, Result, bail};
use futures::channel::oneshot;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST: &str = "https://api.github.com/repos/LuminaX-Digital/notary/releases/latest";

pub struct Release {
    pub version: String,
    pub url: String,
}

/// Asks GitHub for the latest release on a background thread. `Ok(None)` means this version is current.
pub fn check() -> oneshot::Receiver<Result<Option<Release>>> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(fetch());
    });
    rx
}

fn fetch() -> Result<Option<Release>> {
    let mut response = match ureq::get(LATEST)
        .header("User-Agent", format!("Notary/{CURRENT}"))
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => bail!("No release has been published on GitHub yet"),
        Err(ureq::Error::StatusCode(code)) => bail!("github.com answered with status {code}"),
        Err(e) => return Err(anyhow::Error::new(e).context("could not reach github.com. Check your internet connection")),
    };
    let body = response.body_mut().read_to_string().context("could not read the answer from github.com")?;
    parse(&body, CURRENT)
}

fn parse(body: &str, current: &str) -> Result<Option<Release>> {
    let json: serde_json::Value = serde_json::from_str(body).context("github.com sent an unexpected answer")?;
    let tag = json["tag_name"].as_str().context("the release has no version")?;
    let url = json["html_url"].as_str().unwrap_or("https://github.com/LuminaX-Digital/notary/releases/latest");
    let version = tag.trim_start_matches('v');
    if !url.starts_with("https://github.com/") {
        bail!("the release link looks wrong");
    }
    Ok(newer(version, current).then(|| Release { version: version.to_string(), url: url.to_string() }))
}

fn numbers(v: &str) -> Vec<u64> {
    v.split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

/// True when `latest` is a higher version than `current`.
fn newer(latest: &str, current: &str) -> bool {
    let (a, b) = (numbers(latest), numbers(current));
    (0..a.len().max(b.len())).map(|i| (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0))).find(|(x, y)| x != y).is_some_and(|(x, y)| x > y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_by_number() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.5"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        assert!(newer("0.1.1", "0.1"));
    }

    #[test]
    fn reads_a_github_answer() {
        let body = r#"{"tag_name":"v0.3.0","html_url":"https://github.com/LuminaX-Digital/notary/releases/tag/v0.3.0"}"#;
        let r = parse(body, "0.2.0").unwrap().unwrap();
        assert_eq!(r.version, "0.3.0");
        assert!(parse(body, "0.3.0").unwrap().is_none());
    }

    #[test]
    fn rejects_garbage_and_foreign_links() {
        assert!(parse("not json", "0.1.0").is_err());
        assert!(parse(r#"{"tag_name":"v9.0.0","html_url":"https://evil.example/x"}"#, "0.1.0").is_err());
    }
}
