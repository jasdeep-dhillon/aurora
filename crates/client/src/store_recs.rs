//! What the store recommends to you, from Steam's own personal sources:
//! discovery queues (built from what you play, wishlist and ignore), games
//! trending among your friends, and the tags you like most.

use model::{AppId, StoreRecommendations};
use net::Connection;
use proto::*;

/// Steam's queue types (`EStoreDiscoveryQueueType`) the store shows, with
/// the shelf id the app uses for each.
pub const QUEUES: [(i32, &str); 4] = [
    (5, "ml"),         // MLRecommender: "Recommended for you"
    (11, "on_sale"),   // RecommendedOnSale
    (10, "free"),      // RecommendedFree
    (9, "coming_soon"), // RecommendedComingSoon
];

async fn queue(conn: &Connection, kind: i32) -> Vec<AppId> {
    let req = CStoreGetDiscoveryQueueRequest {
        queue_type: Some(kind),
        rebuild_queue_if_stale: Some(true),
        settings: Some(CStoreDiscoveryQueueSettings { exclude_soundtracks: Some(true), exclude_videos: Some(true), exclude_software: Some(true), exclude_dlc: Some(true), ..Default::default() }),
        ..Default::default()
    };
    match conn.call::<_, CStoreGetDiscoveryQueueResponse>("Store.GetDiscoveryQueue#1", &req).await {
        Ok(r) => r.appids,
        Err(e) => {
            tracing::warn!("store: discovery queue {kind} failed: {e}");
            Vec::new()
        }
    }
}

/// Everything at once; a source that fails is left empty.
pub async fn load(conn: &Connection, language: &str) -> StoreRecommendations {
    let queues = queues(conn).await;
    let friends = conn
        .call::<_, CStoreGetTrendingAppsAmongFriendsResponse>("Store.GetTrendingAppsAmongFriends#1", &CStoreGetTrendingAppsAmongFriendsRequest { num_apps: Some(24), num_top_friends: Some(3) })
        .await
        .map(|r| r.trending_apps.iter().map(|a| (a.appid(), a.total_friends())).collect())
        .unwrap_or_default();
    let tags = conn
        .call::<_, CStoreGetRecommendedTagsForUserResponse>(
            "Store.GetRecommendedTagsForUser#1",
            &CStoreGetRecommendedTagsForUserRequest { language: Some(language.to_string()), country_code: None, favor_rarer_tags: Some(false) },
        )
        .await
        .map(|r| r.tags.iter().map(|t| (t.tagid(), t.name().to_string())).collect())
        .unwrap_or_default();
    StoreRecommendations { queues, friends, tags }
}

async fn queues(conn: &Connection) -> Vec<(String, Vec<AppId>)> {
    let mut out = Vec::new();
    // Steam builds each queue on its side; asking one after another keeps
    // the connection's request queue short.
    for (kind, id) in QUEUES {
        out.push((id.to_string(), queue(conn, kind).await));
    }
    out
}
