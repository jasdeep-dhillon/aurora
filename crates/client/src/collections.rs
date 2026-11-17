//! Library collections kept in step with Steam.
//!
//! Steam stores them in the CloudConfigStore, namespace 1, one entry per
//! collection: key `user-collections.<id>`, value JSON
//! `{"id":…,"name":…,"added":[app ids],"removed":[…]}`. Favorites and
//! Hidden are the entries `favorite` and `hidden`; collections the player
//! made are `uc-<random>`. Dynamic collections (a `filterSpec` instead of a
//! list) are only read, never written.
//!
//! The library keeps the state it last agreed on with Steam (the "base").
//! At sign-in it merges base, this PC and Steam three ways, so changes made
//! on either side since then both survive; after that, anything this PC
//! changes is uploaded.

use model::AppId;
use net::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The Steam namespace that holds library collections.
const NAMESPACE: u32 = 1;

/// One collection's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub games: BTreeSet<AppId>,
}

/// Collections by Steam id ("favorite", "hidden", "uc-…").
pub type Collections = BTreeMap<String, Entry>;

/// A new id for a collection made here, in Steam's own form.
pub fn new_id() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    // splitmix64 over the time and a counter: unique enough for ids that
    // only have to differ within one account.
    let mut x = nanos ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (std::process::id() as u64) << 32;
    let mut id = String::from("uc-");
    for _ in 0..12 {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        id.push(ALPHABET[(z % ALPHABET.len() as u64) as usize] as char);
    }
    id
}

/// Three-way merge of collections: `base` is what this PC and Steam last
/// agreed on. Games added or removed on either side since then are kept;
/// a collection deleted on one side and untouched on the other goes; one
/// new on both sides (no base) gets the games of both.
pub fn merge(base: &Collections, local: &Collections, remote: &Collections) -> Collections {
    let ids: BTreeSet<&String> = base.keys().chain(local.keys()).chain(remote.keys()).collect();
    let mut out = Collections::new();
    for id in ids {
        let (b, l, r) = (base.get(id), local.get(id), remote.get(id));
        let merged = match (b, l, r) {
            (_, None, None) => None,
            // New on one side.
            (None, Some(x), None) | (None, None, Some(x)) => Some(x.clone()),
            // Deleted on one side: gone, unless the other side changed it.
            (Some(b), Some(x), None) | (Some(b), None, Some(x)) => (x != b).then(|| x.clone()),
            (None, Some(l), Some(r)) => Some(Entry { name: if l.name.is_empty() { r.name.clone() } else { l.name.clone() }, games: l.games.union(&r.games).copied().collect() }),
            (Some(b), Some(l), Some(r)) => {
                let mut games: BTreeSet<AppId> = b.games.clone();
                for side in [l, r] {
                    games.extend(side.games.difference(&b.games));
                }
                for side in [l, r] {
                    for gone in b.games.difference(&side.games) {
                        games.remove(gone);
                    }
                }
                let name = if l.name != b.name { l.name.clone() } else { r.name.clone() };
                Some(Entry { name, games })
            }
        };
        if let Some(m) = merged {
            out.insert(id.clone(), m);
        }
    }
    out
}

/// What to send so Steam matches `local`: (changed or new, deleted ids).
pub fn diff(base: &Collections, local: &Collections) -> (Vec<(String, Entry)>, Vec<String>) {
    let save = local.iter().filter(|(id, e)| base.get(*id) != Some(e)).map(|(id, e)| (id.clone(), e.clone())).collect();
    // Favorites and Hidden always exist in Steam; never delete them.
    let delete = base.keys().filter(|id| !local.contains_key(*id) && !matches!(id.as_str(), "favorite" | "hidden")).cloned().collect();
    (save, delete)
}

/// The JSON Steam stores for a collection, keeping any fields Steam put in
/// the old value that we don't know about.
pub fn value(old: Option<&str>, id: &str, e: &Entry) -> String {
    let mut map: Map<String, Value> = old.and_then(|o| serde_json::from_str::<Value>(o).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    map.insert("id".into(), Value::from(id));
    // Favorites and Hidden have Steam's own names; keep them.
    if !e.name.is_empty() || !map.contains_key("name") {
        map.insert("name".into(), Value::from(e.name.clone()));
    }
    map.insert("added".into(), Value::from(e.games.iter().copied().collect::<Vec<AppId>>()));
    map.insert("removed".into(), Value::Array(Vec::new()));
    map.remove("filterSpec");
    Value::Object(map).to_string()
}

/// Write collections to Steam: `save` are created or replaced, `delete`
/// removed. Reads the namespace first so each entry goes up with the
/// version Steam expects.
pub async fn save(conn: &Connection, save: &[(String, Entry)], delete: &[String]) -> Result<(), String> {
    use proto::webui::*;
    let current: CCloudConfigStoreDownloadResponse = conn
        .call("CloudConfigStore.Download#1", &CCloudConfigStoreDownloadRequest { versions: vec![CCloudConfigStoreNamespaceVersion { enamespace: Some(NAMESPACE), version: Some(0) }] })
        .await
        .map_err(|e| e.to_string())?;
    let data = current.data.iter().find(|d| d.enamespace() == NAMESPACE);
    let known: BTreeMap<&str, &CCloudConfigStoreEntry> = data.map(|d| d.entries.iter().map(|e| (e.key(), e)).collect()).unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
    let mut entries = Vec::new();
    for (id, e) in save {
        let key = format!("user-collections.{id}");
        let old = known.get(key.as_str());
        entries.push(CCloudConfigStoreEntry {
            value: Some(value(old.filter(|o| !o.is_deleted()).map(|o| o.value()), id, e)),
            key: Some(key),
            is_deleted: Some(false),
            timestamp: Some(now),
            version: old.and_then(|o| o.version),
        });
    }
    for id in delete {
        let key = format!("user-collections.{id}");
        let Some(old) = known.get(key.as_str()) else { continue };
        entries.push(CCloudConfigStoreEntry { key: Some(key), is_deleted: Some(true), value: None, timestamp: Some(now), version: old.version });
    }
    if entries.is_empty() {
        return Ok(());
    }
    let req = CCloudConfigStoreUploadRequest { data: vec![CCloudConfigStoreNamespaceData { enamespace: Some(NAMESPACE), version: data.and_then(|d| d.version), entries, horizon: None }] };
    conn.call::<_, CCloudConfigStoreUploadResponse>("CloudConfigStore.Upload#1", &req).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, games: &[AppId]) -> Entry {
        Entry { name: name.into(), games: games.iter().copied().collect() }
    }
    fn map(items: &[(&str, Entry)]) -> Collections {
        items.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn merges_changes_from_both_sides() {
        let base = map(&[("favorite", e("", &[1, 2])), ("uc-a", e("A", &[10])), ("uc-gone-here", e("G", &[5])), ("uc-gone-there", e("T", &[6]))]);
        let local = map(&[("favorite", e("", &[1, 2, 3])), ("uc-a", e("A2", &[10])), ("uc-gone-there", e("T", &[6])), ("uc-new-here", e("N", &[7]))]);
        let remote = map(&[("favorite", e("", &[2, 4])), ("uc-a", e("A", &[10, 11])), ("uc-gone-here", e("G", &[5])), ("uc-new-there", e("M", &[8]))]);
        let m = merge(&base, &local, &remote);
        // 3 added here, 4 added there, 1 removed there.
        assert_eq!(m["favorite"], e("", &[2, 3, 4]));
        // Renamed here, game added there.
        assert_eq!(m["uc-a"], e("A2", &[10, 11]));
        assert!(!m.contains_key("uc-gone-here"));
        assert!(!m.contains_key("uc-gone-there"));
        assert_eq!(m["uc-new-here"], e("N", &[7]));
        assert_eq!(m["uc-new-there"], e("M", &[8]));
    }

    #[test]
    fn first_sync_unites_both_sides() {
        let local = map(&[("favorite", e("", &[1]))]);
        let remote = map(&[("favorite", e("Favorites", &[2]))]);
        assert_eq!(merge(&Collections::new(), &local, &remote)["favorite"], e("Favorites", &[1, 2]));
    }

    #[test]
    fn diff_finds_changes_and_deletes_but_keeps_favorites() {
        let base = map(&[("favorite", e("", &[1])), ("hidden", e("", &[])), ("uc-a", e("A", &[1])), ("uc-b", e("B", &[2]))]);
        let local = map(&[("favorite", e("", &[1, 2])), ("uc-a", e("A", &[1]))]);
        let (save, delete) = diff(&base, &local);
        assert_eq!(save, vec![("favorite".to_string(), e("", &[1, 2]))]);
        assert_eq!(delete, vec!["uc-b".to_string()]);
    }

    #[test]
    fn value_keeps_unknown_fields_and_steam_names() {
        let v = value(Some(r#"{"id":"favorite","name":"Favorites","added":[1],"removed":[2],"extra":true}"#), "favorite", &e("", &[3, 1]));
        let v: Value = serde_json::from_str(&v).unwrap();
        assert_eq!(v["name"], "Favorites");
        assert_eq!(v["added"], serde_json::json!([1, 3]));
        assert_eq!(v["removed"], serde_json::json!([]));
        assert_eq!(v["extra"], true);
    }

    #[test]
    fn ids_look_like_steams() {
        let (a, b) = (new_id(), new_id());
        assert!(a.starts_with("uc-") && a.len() == 15);
        assert_ne!(a, b);
    }
}
