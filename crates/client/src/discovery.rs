//! The store's discovery queue and follow controls, adapted from upstream
//! work by `madonchik123` (PR #62 `731cef2`): a dozen games picked for you,
//! gone through one at a time, like Steam's.

use model::{AppId, SteamId};
use net::Connection;

/// Your discovery queue (Steam keeps it until you've been through it; with
/// `rebuild` you get a fresh one).
pub async fn queue(conn: &Connection, country: &str, rebuild: bool) -> Result<Vec<AppId>, String> {
    if rebuild {
        // Like going through the queue on Steam: the games in it count as
        // seen, so the new set doesn't hand the same ones back.
        for app_id in fetch(conn, country, false).await.unwrap_or_default() {
            let _ = skip(conn, app_id).await;
        }
    }
    fetch(conn, country, rebuild).await
}

async fn fetch(conn: &Connection, country: &str, rebuild: bool) -> Result<Vec<AppId>, String> {
    let req = proto::CStoreGetDiscoveryQueueRequest {
        queue_type: Some(proto::EStoreDiscoveryQueueType::KEStoreDiscoveryQueueTypeNew as i32),
        country_code: (!country.is_empty()).then(|| country.to_string()),
        rebuild_queue: Some(rebuild),
        rebuild_queue_if_stale: Some(true),
        ..Default::default()
    };
    let r: proto::CStoreGetDiscoveryQueueResponse = conn.call("Store.GetDiscoveryQueue#1", &req).await.map_err(|e| e.to_string())?;
    Ok(r.appids)
}

/// Mark a game in the queue as seen (Steam's "Next in queue").
pub async fn skip(conn: &Connection, app_id: AppId) -> Result<(), String> {
    let req = proto::CStoreSkipDiscoveryQueueItemRequest {
        queue_type: Some(proto::EStoreDiscoveryQueueType::KEStoreDiscoveryQueueTypeNew as i32),
        appid: Some(app_id),
        ..Default::default()
    };
    match conn.call::<_, proto::CStoreSkipDiscoveryQueueItemResponse>("Store.SkipDiscoveryQueueItem#1", &req).await {
        Ok(_) => Ok(()),
        // EResult::DuplicateRequest (29) means this queue item was already marked skipped (e.g. Previous -> Next).
        Err(e) if e.to_string().contains("Steam error 29") => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Build the form parameters for Steam's "Not interested" endpoint.
pub(crate) fn ignore_form(session_id: &str, app_id: AppId, ignore: bool) -> Vec<(&'static str, String)> {
    let mut form = vec![
        ("sessionid", session_id.to_string()),
        ("appid", app_id.to_string()),
        ("snr", "1_4_4__100".to_string()),
    ];
    if ignore {
        form.push(("ignore_reason", "0".to_string()));
    } else {
        form.push(("remove", "1".to_string()));
    }
    form
}

/// "Not interested" (or undo it): Steam stops recommending the game.
pub async fn ignore(web: &crate::market::Web, app_id: AppId, ignore: bool) -> Result<(), String> {
    let form = ignore_form(&web.session_id, app_id, ignore);
    let r = web
        .http
        .post("https://store.steampowered.com/recommended/ignorerecommendation/")
        .header("Origin", "https://store.steampowered.com")
        .header("Referer", format!("https://store.steampowered.com/app/{app_id}/"))
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("Steam answered {}", r.status()));
    }
    Ok(())
}

/// Games the account follows in the store.
pub async fn followed(conn: &Connection, me: SteamId) -> Result<Vec<AppId>, String> {
    let req = proto::CStoreGetGamesFollowedRequest { steamid: Some(me) };
    let r: proto::CStoreGetGamesFollowedResponse = conn.call("Store.GetGamesFollowed#1", &req).await.map_err(|e| e.to_string())?;
    Ok(r.appids)
}

/// Build the form parameters for Steam's store follow toggle endpoint.
pub(crate) fn follow_form(session_id: &str, app_id: AppId, follow: bool) -> Vec<(&'static str, String)> {
    let mut form = vec![("sessionid", session_id.to_string()), ("appid", app_id.to_string())];
    if !follow {
        form.push(("unfollow", "1".to_string()));
    }
    form
}

/// Steam returns `true` or `{"success":true}` / `{"success":1}` on `/explore/followgame/`.
pub(crate) fn parse_follow_response(v: &serde_json::Value) -> bool {
    v.as_bool() == Some(true)
        || v.get("success").and_then(|s| s.as_bool()) == Some(true)
        || v.get("success").and_then(|s| s.as_i64()) == Some(1)
}

/// Follow or unfollow a game's store hub.
pub async fn follow(web: &crate::market::Web, app_id: AppId, follow: bool) -> Result<bool, String> {
    let form = follow_form(&web.session_id, app_id, follow);
    let r = web
        .http
        .post("https://store.steampowered.com/explore/followgame/")
        .header("Origin", "https://store.steampowered.com")
        .header("Referer", format!("https://store.steampowered.com/app/{app_id}/"))
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("Steam answered {}", r.status()));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
    if parse_follow_response(&v) {
        Ok(follow)
    } else {
        Err("Couldn't update follow status. Try again in a moment.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_ignore_and_follow_forms() {
        let ig = ignore_form("sess", 730, true);
        assert_eq!(ig[1], ("appid", "730".to_string()));
        assert_eq!(ig[3], ("ignore_reason", "0".to_string()));
        let unig = ignore_form("sess", 730, false);
        assert_eq!(unig[3], ("remove", "1".to_string()));

        assert_eq!(follow_form("sess", 570, true), vec![("sessionid", "sess".to_string()), ("appid", "570".to_string())]);
        assert_eq!(
            follow_form("sess", 570, false),
            vec![("sessionid", "sess".to_string()), ("appid", "570".to_string()), ("unfollow", "1".to_string())]
        );
    }

    #[test]
    fn parses_follow_answers() {
        assert!(parse_follow_response(&json!(true)));
        assert!(parse_follow_response(&json!({ "success": true })));
        assert!(parse_follow_response(&json!({ "success": 1 })));
        assert!(!parse_follow_response(&json!(false)));
        assert!(!parse_follow_response(&json!({ "success": false })));
    }
}
