//! Secrets stored in the macOS Keychain (never in the settings database).
//!
//! Currently only the Ollama Cloud API key. Reads are cached for the process
//! lifetime so the Keychain is not queried for every request; writes update
//! the cache.

use std::sync::Mutex;

/// Keychain item service name of this app.
pub const SERVICE: &str = "Meeting Notes AI";
/// Keychain account holding the Ollama Cloud API key.
pub const OLLAMA_CLOUD_ACCOUNT: &str = "Ollama Cloud API key";

#[cfg(target_os = "macos")]
mod platform {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    /// `errSecItemNotFound`
    const ITEM_NOT_FOUND: i32 = -25300;

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        match get_generic_password(service, account) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "Keychain item is not valid UTF-8".to_string()),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
            Err(error) => Err(format!("Keychain read failed: {error}")),
        }
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        set_generic_password(service, account, value.as_bytes())
            .map_err(|error| format!("Keychain write failed: {error}"))
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        match delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(()),
            Err(error) => Err(format!("Keychain delete failed: {error}")),
        }
    }
}

/// Other platforms (tests on Linux CI): an in-memory store.
#[cfg(not(target_os = "macos"))]
mod platform {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static STORE: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);

    fn with_store<T>(f: impl FnOnce(&mut HashMap<(String, String), String>) -> T) -> T {
        let mut guard = STORE.lock().unwrap_or_else(|poison| poison.into_inner());
        f(guard.get_or_insert_with(HashMap::new))
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        Ok(with_store(|s| {
            s.get(&(service.into(), account.into())).cloned()
        }))
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        with_store(|s| s.insert((service.into(), account.into()), value.into()));
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        with_store(|s| s.remove(&(service.into(), account.into())));
        Ok(())
    }
}

/// `None` = not read yet; `Some(None)` = read, no key stored.
static OLLAMA_CLOUD_KEY_CACHE: Mutex<Option<Option<String>>> = Mutex::new(None);

fn cache() -> std::sync::MutexGuard<'static, Option<Option<String>>> {
    OLLAMA_CLOUD_KEY_CACHE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// The stored Ollama Cloud API key, if any. Keychain errors are logged and
/// treated as "no key" so a locked Keychain never crashes a summary.
pub fn ollama_cloud_api_key() -> Option<String> {
    let mut cached = cache();
    if let Some(value) = cached.as_ref() {
        return value.clone();
    }
    let value = match platform::get(SERVICE, OLLAMA_CLOUD_ACCOUNT) {
        Ok(value) => value.filter(|key| !key.trim().is_empty()),
        Err(error) => {
            tracing::warn!(%error, "Could not read the Ollama Cloud API key");
            return None;
        }
    };
    *cached = Some(value.clone());
    value
}

pub fn has_ollama_cloud_api_key() -> bool {
    ollama_cloud_api_key().is_some()
}

/// Store the key (trimmed); an empty key deletes the Keychain item.
pub fn set_ollama_cloud_api_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        platform::delete(SERVICE, OLLAMA_CLOUD_ACCOUNT)?;
        *cache() = Some(None);
    } else {
        platform::set(SERVICE, OLLAMA_CLOUD_ACCOUNT, key)?;
        *cache() = Some(Some(key.to_string()));
    }
    Ok(())
}

// The real Keychain is not touched from tests; the in-memory store only
// exists off macOS.
#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn store_read_and_delete_key() {
        set_ollama_cloud_api_key("  secret-key  ").unwrap();
        assert_eq!(ollama_cloud_api_key().as_deref(), Some("secret-key"));
        assert!(has_ollama_cloud_api_key());
        set_ollama_cloud_api_key("").unwrap();
        assert_eq!(ollama_cloud_api_key(), None);
    }
}
