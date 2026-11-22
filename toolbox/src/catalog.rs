//! "Get programs" (DESIGN.md 9, 15.5): the list of known programs, `programs.json` in this repo, signed with the
//! release key. Loaded only when the list is opened – never on its own. A click on one installs it into
//! `programs\<id>\` next to `Toolbox.exe` with the same code as an update, checked against the key the signed list
//! names for it.

use std::path::{Path, PathBuf};

use kit::manifest::Text;
use kit::{http, update};
use serde::{Deserialize, Serialize};

/// Where the list lies: the `main` branch of this repo. The signature lies next to it.
pub const URL: &str = "https://raw.githubusercontent.com/Windows-Toolbox/Toolbox/main/programs.json";

/// One program on the list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Offer {
    pub id: String,
    pub name: Text,
    #[serde(default)]
    pub description: Text,
    /// `owner/name` on GitHub – where its releases are.
    pub repository: String,
    /// The key its releases are signed with (base64, 32 bytes).
    pub update_key: String,
}

#[derive(Deserialize)]
struct List {
    programs: Vec<Offer>,
}

/// The same rule as a manifest's id: it becomes a folder name.
fn is_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Checks the signature over the list's exact bytes against `key`, then reads it. Entries with an id that could
/// not be a folder are left out.
pub fn parse(json: &[u8], signature: &str, key: &str) -> Result<Vec<Offer>, String> {
    update::verify(json, signature, key).map_err(|e| format!("programs.json: {e}"))?;
    let list: List = serde_json::from_slice(json).map_err(|e| format!("programs.json cannot be read: {e}"))?;
    Ok(list.programs.into_iter().filter(|o| is_id(&o.id) && o.id != crate::library::OWN_ID).collect())
}

/// Loads the list from GitHub and checks it against the key compiled into the Toolbox.
pub fn load() -> Result<Vec<Offer>, String> {
    const LIMIT: usize = 1024 * 1024;
    let fetch = |url: &str| match http::get(url, &[], LIMIT) {
        Ok(answer) if answer.ok() => Ok(answer.body),
        Ok(answer) => Err(format!("{url}: the server answered {}", answer.status)),
        Err(e) => Err(format!("GitHub cannot be reached: {e}")),
    };
    let json = fetch(URL)?;
    let signature = fetch(&format!("{URL}.sig"))?;
    parse(&json, &String::from_utf8_lossy(&signature), update::KEY)
}

/// Where "Get" puts a program: `programs\<id>\` next to `Toolbox.exe`.
pub fn folder(base: &Path, id: &str) -> PathBuf {
    base.join("programs").join(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn only_a_signed_list_is_read() {
        let secret = SigningKey::from_bytes(&[3u8; 32]);
        let key = update::base64_encode(&secret.verifying_key().to_bytes());
        let json = br#"{"programs":[
            {"id":"screen_zones","name":{"de":"Screen Zones","en":"Screen Zones"},"description":"Zones","repository":"Windows-Toolbox/Screen-Zones","update_key":"abc"},
            {"id":"..\\evil","name":"Evil","repository":"x/y","update_key":"abc"},
            {"id":"toolbox","name":"Toolbox","repository":"Windows-Toolbox/Toolbox","update_key":"abc"},
            {"id":"plain","name":"Plain","repository":"o/r","update_key":"abc"}]}"#;
        let signature = update::base64_encode(&secret.sign(json).to_bytes());
        let offers = parse(json, &signature, &key).unwrap();
        assert_eq!(offers.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["screen_zones", "plain"], "no odd ids, not the Toolbox itself");
        assert_eq!((offers[0].name.get("de"), offers[1].description.is_empty()), ("Screen Zones", true));

        let mut changed = json.to_vec();
        changed[20] ^= 1;
        assert!(parse(&changed, &signature, &key).unwrap_err().contains("signature"));
        let other = update::base64_encode(&SigningKey::from_bytes(&[4u8; 32]).verifying_key().to_bytes());
        assert!(parse(json, &signature, &other).is_err());
        assert_eq!(folder(Path::new(r"C:\T"), "screen_zones"), Path::new(r"C:\T\programs\screen_zones"));
    }

    /// The list in this repo is what the Toolbox will load: it must be signed with the key compiled in, and name
    /// only programs whose releases that key checks.
    #[test]
    fn the_list_in_this_repo_is_signed() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let json = std::fs::read(root.join("programs.json")).unwrap();
        let signature = std::fs::read_to_string(root.join("programs.json.sig")).expect("programs.json.sig – run: cargo run -p sign -- file programs.json");
        let offers = parse(&json, &signature, update::KEY).expect("programs.json changed after it was signed – sign it again");
        assert!(!offers.is_empty());
        for o in &offers {
            assert_eq!(o.update_key, update::KEY, "{}: kit programs are signed with the kit's key", o.id);
        }
    }
}
