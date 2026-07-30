//! The player's account, as the UI sees it.
//!
//! The key is generated here (the browser has `crypto.getRandomValues`; the
//! delegate has no randomness source) and handed to the delegate to store. A
//! `localStorage` copy is kept as a fallback so the app still works before the
//! delegate has answered, or if it is unavailable — losing your identity on a
//! refresh would be far worse than the modest exposure of a key that the page
//! holds in memory anyway.

use chess_core::account;
use chess_core::identity::PlayerId;
use ed25519_dalek::SigningKey;

const STORAGE_KEY: &str = "freechess:account:v1";
const STORAGE_NICK: &str = "freechess:nickname:v1";

/// The signed-in player.
#[derive(Clone)]
pub struct Account {
    pub key: SigningKey,
    pub nickname: String,
}

impl Account {
    pub fn id(&self) -> PlayerId {
        PlayerId::from(&self.key.verifying_key())
    }

    /// The portable account string the user can copy out.
    pub fn export(&self) -> String {
        account::export(&self.key)
    }

    /// A readable default name for a brand-new account.
    pub fn default_nickname(id: PlayerId) -> String {
        format!("player-{}", &id.to_base58()[..6])
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Read the locally cached account, if any.
pub fn load_local() -> Option<SigningKey> {
    let raw = storage()?.get_item(STORAGE_KEY).ok().flatten()?;
    account::import(&raw).ok()
}

pub fn save_local(key: &SigningKey) {
    if let Some(store) = storage() {
        let _ = store.set_item(STORAGE_KEY, &account::export(key));
    }
}

pub fn load_local_nickname() -> Option<String> {
    storage()?.get_item(STORAGE_NICK).ok().flatten()
}

pub fn save_local_nickname(nickname: &str) {
    if let Some(store) = storage() {
        let _ = store.set_item(STORAGE_NICK, nickname);
    }
}

pub fn clear_local() {
    if let Some(store) = storage() {
        let _ = store.remove_item(STORAGE_KEY);
        let _ = store.remove_item(STORAGE_NICK);
    }
}

/// Mint a fresh account.
pub fn generate() -> SigningKey {
    // `OsRng` maps to `crypto.getRandomValues` under the `getrandom/js`
    // feature, which the UI crate enables (and contracts must not).
    SigningKey::generate(&mut rand::rngs::OsRng)
}

/// Load the stored account or create one, returning it with its nickname.
pub fn load_or_create() -> Account {
    let key = match load_local() {
        Some(key) => key,
        None => {
            let key = generate();
            save_local(&key);
            key
        }
    };
    let id = PlayerId::from(&key.verifying_key());
    let nickname = load_local_nickname().unwrap_or_else(|| Account::default_nickname(id));
    Account { key, nickname }
}

/// Current wall-clock time in Unix milliseconds.
///
/// Every signed timestamp in the protocol comes from here. It is the client's
/// own clock, which is why the contracts never trust it on its own: they check
/// ordering and derive elapsed time from *differences* between signed
/// timestamps rather than from any single absolute value.
pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_account_round_trips_through_its_export_string() {
        let key = generate();
        let text = account::export(&key);
        assert_eq!(account::import(&text).unwrap().to_bytes(), key.to_bytes());
    }
}
