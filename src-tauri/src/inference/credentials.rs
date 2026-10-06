pub trait CredentialStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, secret: &str) -> Result<(), String>;
    fn remove(&self, id: &str) -> Result<(), String>;
}
pub struct OsCredentialStore;
impl OsCredentialStore {
    fn entry(id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new("dev.promptify.app.inference", id).map_err(|_| {
            "OS credential store unavailable. Unlock your keychain or Secret Service.".into()
        })
    }
}
impl CredentialStore for OsCredentialStore {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match Self::entry(id)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(
                "Cannot read OS credential store. Unlock your keychain or Secret Service.".into(),
            ),
        }
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        Self::entry(id)?
            .set_password(secret)
            .map_err(|_| "Cannot save credential to OS credential store.".into())
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("Cannot remove credential from OS credential store.".into()),
        }
    }
}
