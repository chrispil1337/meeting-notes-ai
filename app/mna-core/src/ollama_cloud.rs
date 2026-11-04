//! Ollama Cloud: models hosted by Ollama, used directly through ollama.com.
//!
//! Ollama documents direct cloud access through the same native API a local
//! server offers (`/api/tags`, `/api/generate`, ...) at `https://ollama.com`,
//! authenticated with `Authorization: Bearer <API key>`. The key comes from
//! <https://ollama.com/settings/keys>. No local Ollama installation is needed.
//!
//! The API key must never be sent anywhere else, so [`authorization_header`]
//! only returns a header for the official HTTPS host.

/// Base URL of Ollama Cloud's native API (paths are appended as `/api/...`).
pub const OLLAMA_CLOUD_URL: &str = "https://ollama.com";

/// Where users create API keys.
pub const OLLAMA_CLOUD_KEYS_URL: &str = "https://ollama.com/settings/keys";

/// True for the official Ollama Cloud endpoint (HTTPS only, any path).
pub fn is_ollama_cloud_url(url: &str) -> bool {
    let url = url.trim();
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Reject userinfo tricks like https://ollama.com@evil.example
    if host_port.contains('@') {
        return false;
    }
    let host = host_port
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    host == "ollama.com" || host == "www.ollama.com"
}

/// Value of the `Authorization` header for a request to `url`, if any.
///
/// Returns `None` for every URL that is not Ollama Cloud and for an empty
/// key, so a local or self-hosted server never receives the cloud key.
pub fn authorization_header(url: &str, api_key: Option<&str>) -> Option<String> {
    if !is_ollama_cloud_url(url) {
        return None;
    }
    let key = api_key?.trim();
    if key.is_empty() {
        return None;
    }
    Some(format!("Bearer {key}"))
}

/// Some reasoning models (e.g. DeepSeek) write their reasoning into the
/// answer as `<think>...</think>`. Remove those blocks so the rest of the
/// pipeline only sees the answer. An unterminated block at the start is
/// dropped too (the answer then is everything after it, i.e. nothing).
pub fn strip_reasoning(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find("<think>") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_open = start + "<think>".len();
        match lower[after_open..].find("</think>") {
            Some(end) => rest = &rest[after_open + end + "</think>".len()..],
            None => break,
        }
    }
    out.trim().to_string()
}

/// User-facing explanation for an HTTP status from Ollama Cloud.
pub fn describe_cloud_status(status: u16) -> Option<&'static str> {
    match status {
        401 | 403 => Some("Ollama Cloud rejected the API key. Check it in Settings > AI."),
        429 => Some("Ollama Cloud usage limit reached. Try again later or check your plan."),
        404 => Some("This model is not available on Ollama Cloud. Choose another model."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_only_the_official_https_host() {
        assert!(is_ollama_cloud_url("https://ollama.com"));
        assert!(is_ollama_cloud_url("https://ollama.com/"));
        assert!(is_ollama_cloud_url("  https://OLLAMA.com/api  "));
        assert!(is_ollama_cloud_url("https://www.ollama.com"));
        assert!(is_ollama_cloud_url("https://ollama.com:443"));
        assert!(!is_ollama_cloud_url("http://ollama.com"));
        assert!(!is_ollama_cloud_url("http://localhost:11434"));
        assert!(!is_ollama_cloud_url("https://ollama.com.evil.example"));
        assert!(!is_ollama_cloud_url("https://evil.example/ollama.com"));
        assert!(!is_ollama_cloud_url("https://ollama.com@evil.example"));
        assert!(!is_ollama_cloud_url(""));
    }

    #[test]
    fn header_only_for_cloud_and_non_empty_key() {
        assert_eq!(
            authorization_header("https://ollama.com", Some(" key ")),
            Some("Bearer key".to_string())
        );
        assert_eq!(
            authorization_header("http://localhost:11434", Some("key")),
            None
        );
        assert_eq!(authorization_header("https://ollama.com", Some("  ")), None);
        assert_eq!(authorization_header("https://ollama.com", None), None);
    }

    #[test]
    fn reasoning_blocks_are_removed() {
        assert_eq!(
            strip_reasoning("<think>\nhmm\n</think>\n\n{\"a\":1}"),
            "{\"a\":1}"
        );
        assert_eq!(
            strip_reasoning("A<THINK>x</THINK>B<think>y</think>C"),
            "ABC"
        );
        assert_eq!(strip_reasoning("no reasoning"), "no reasoning");
        assert_eq!(strip_reasoning("<think>never closed"), "");
        assert_eq!(strip_reasoning("Ümlaut <think>ä</think> ok"), "Ümlaut  ok");
    }

    #[test]
    fn status_messages() {
        assert!(describe_cloud_status(401).unwrap().contains("API key"));
        assert!(describe_cloud_status(429).is_some());
        assert!(describe_cloud_status(500).is_none());
    }
}
