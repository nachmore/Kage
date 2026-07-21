use anyhow::{Context, Result};

const RELEASE_NOTES_BUDGET: usize = 30 * 1024;
const RELEASE_NOTES_LIMIT: usize = 10;

fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let path = url
        .trim()
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    let mut parts = path.strip_suffix(".git").unwrap_or(path).splitn(2, '/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    (!owner.is_empty() && !repo.is_empty()).then(|| (owner.into(), repo.into()))
}

fn format_release_date(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| date.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|_| value.into())
}

/// Fetch recent GitHub release notes appropriate for the selected channel.
pub fn fetch_changelog(channel: crate::config::Channel) -> Result<String> {
    let repository = env!("CARGO_PKG_REPOSITORY");
    let Some((owner, repo)) = parse_github_repo(repository) else {
        return Ok(format!(
            "No GitHub repository configured (got `{repository}`). Release notes are unavailable."
        ));
    };
    let response = reqwest::blocking::Client::new()
        .get(format!(
            "https://api.github.com/repos/{owner}/{repo}/releases?per_page=30"
        ))
        .header(
            "User-Agent",
            format!("Kage/{}", super::checks::CURRENT_VERSION),
        )
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .context("Failed to reach GitHub releases API")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if status.as_u16() == 403 && body.to_lowercase().contains("rate limit") {
            return Ok("GitHub API rate limit reached. Please try again in an hour, or view release notes on GitHub directly.".into());
        }
        return Err(anyhow::anyhow!(
            "GitHub API returned {}: {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }
    let releases: Vec<serde_json::Value> = response
        .json()
        .context("Failed to parse GitHub releases JSON")?;
    let prereleases = channel != crate::config::Channel::Stable;
    let mut rendered = String::new();
    for release in releases
        .iter()
        .filter(|release| {
            !release
                .get("draft")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                && (prereleases
                    || !release
                        .get("prerelease")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false))
        })
        .take(RELEASE_NOTES_LIMIT)
    {
        let name = release
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .or_else(|| release.get("tag_name").and_then(|value| value.as_str()))
            .unwrap_or("(untitled)");
        rendered.push_str("## ");
        rendered.push_str(name);
        if let Some(date) = release.get("published_at").and_then(|value| value.as_str()) {
            rendered.push_str(" - ");
            rendered.push_str(&format_release_date(date));
        }
        if release
            .get("prerelease")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            rendered.push_str(" *(prerelease)*");
        }
        rendered.push('\n');
        if let Some(url) = release
            .get("html_url")
            .and_then(|value| value.as_str())
            .filter(|url| !url.is_empty())
        {
            rendered.push_str(&format!("[View on GitHub]({url})\n\n"));
        } else {
            rendered.push('\n');
        }
        let body = release
            .get("body")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        rendered.push_str(if body.is_empty() {
            "_No release notes._"
        } else {
            body
        });
        rendered.push_str("\n\n---\n\n");
        if rendered.len() >= RELEASE_NOTES_BUDGET {
            let mut end = RELEASE_NOTES_BUDGET;
            while end > 0 && !rendered.is_char_boundary(end) {
                end -= 1;
            }
            rendered.truncate(end);
            rendered.push_str("\n\n*Older releases truncated. View the full history on GitHub.*\n");
            break;
        }
    }
    if rendered.is_empty() {
        Ok(format!(
            "No releases found for the **{}** channel yet.",
            channel.as_str()
        ))
    } else {
        Ok(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_github_repo;
    #[test]
    fn parses_github_urls() {
        assert_eq!(
            parse_github_repo("https://github.com/nachmore/Kage"),
            Some(("nachmore".into(), "Kage".into()))
        );
        assert_eq!(
            parse_github_repo("https://github.com/nachmore/Kage/"),
            Some(("nachmore".into(), "Kage".into()))
        );
        assert_eq!(
            parse_github_repo("https://github.com/nachmore/Kage.git"),
            Some(("nachmore".into(), "Kage".into()))
        );
        assert_eq!(
            parse_github_repo("git@github.com:nachmore/Kage.git"),
            Some(("nachmore".into(), "Kage".into()))
        );
        assert_eq!(parse_github_repo("https://gitlab.com/foo/bar"), None);
        assert_eq!(parse_github_repo(""), None);
        assert_eq!(parse_github_repo("https://github.com/"), None);
        assert_eq!(parse_github_repo("https://github.com/onlyowner"), None);
    }
}
