use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAccount {
    pub player_id: String,
    pub refresh_token: String,
}

pub trait AccountStore: Send + Sync {
    fn load(&self) -> Option<StoredAccount>;
    fn save(&self, account: &StoredAccount);
    fn delete(&self);
}

pub struct KeyringAccountStore;

const SERVICE: &str = "HyperFarm Launcher";
const USER: &str = "microsoft";

impl KeyringAccountStore {
    fn entry() -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, USER)
            .inspect_err(|error| log::warn!("The credential store is unavailable: {error}"))
            .ok()
    }
}

impl AccountStore for KeyringAccountStore {
    fn load(&self) -> Option<StoredAccount> {
        let stored = Self::entry()?.get_password().ok()?;
        match serde_json::from_str(&stored) {
            Ok(account) => Some(account),
            Err(error) => {
                log::warn!("Discarding an unreadable stored account: {error}");
                self.delete();
                None
            }
        }
    }

    fn save(&self, account: &StoredAccount) {
        let Some(entry) = Self::entry() else { return };
        let json = serde_json::to_string(account).expect("a stored account always serializes");
        if let Err(error) = entry.set_password(&json) {
            log::warn!(
                "Could not store the Microsoft account; the next launch signs in again: {error}"
            );
        }
    }

    fn delete(&self) {
        if let Some(entry) = Self::entry() {
            let _ = entry.delete_credential();
        }
    }
}
