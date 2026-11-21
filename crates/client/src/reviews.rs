//! Game reviews: what players say about a game (Steam's Web API
//! `IUserReviewsService/GetAppReviews`, which replaced the store's
//! `appreviews` JSON on 22 October 2026; no key needed), and writing,
//! editing or deleting your own (Steam's `recommendgame` form, through the
//! signed-in web session).

use crate::market::Web;
use model::{AppId, AppReview, AppReviews, ReviewFilter, ReviewKind, ReviewOrder, ReviewWindow, UserReview};
use serde_json::Value;

const STORE: &str = "https://store.steampowered.com";
const API: &str = "https://api.steampowered.com";
const COMMUNITY: &str = "https://steamcommunity.com";

/// Build the `input_json` query payload for `IUserReviewsService/GetAppReviews/v1/`.
pub fn request_input(app_id: AppId, language: &str, cursor: &str, filter: ReviewFilter) -> Value {
    let sort = match filter.order {
        ReviewOrder::Helpful => 0,
        ReviewOrder::Recent => 1,
        ReviewOrder::Funny => 3,
    };
    let day_range = match filter.order {
        ReviewOrder::Helpful => match filter.window {
            ReviewWindow::AllTime => 0,
            ReviewWindow::Day => 1,
            ReviewWindow::Week => 7,
            ReviewWindow::Month => 30,
            ReviewWindow::ThreeMonths => 90,
            ReviewWindow::SixMonths => 180,
            ReviewWindow::Year => 365,
        },
        _ => 0,
    };
    let review_type = match filter.kind {
        ReviewKind::All => 0,
        ReviewKind::Positive => 1,
        ReviewKind::Negative => 2,
    };
    let lang = if filter.all_languages { "all" } else { language };
    serde_json::json!({
        "appid": app_id,
        "filter": sort,
        "day_range": day_range,
        "languages": [lang],
        "display_language": i18n_api_language(lang),
        // All reviews, also from keys bought elsewhere.
        "review_type": review_type,
        "purchase_type": 1,
        "num_per_page": 20,
        "cursor": cursor,
    })
}

/// One page (20) of a game's reviews, most helpful first. `cursor`
/// is the one the last page returned ("*" for the first page). `language`
/// is a Steam API language name ("english", "russian"…), or "all".
pub async fn load(app_id: AppId, language: &str, cursor: &str) -> Result<AppReviews, String> {
    load_filtered(app_id, language, cursor, ReviewFilter::default()).await
}

/// Filtered variant of [`load`].
pub async fn load_filtered(app_id: AppId, language: &str, cursor: &str, filter: ReviewFilter) -> Result<AppReviews, String> {
    let input = request_input(app_id, language, cursor, filter);
    let resp = crate::http()
        .get(format!("{API}/IUserReviewsService/GetAppReviews/v1/"))
        .query(&[("input_json", input.to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("Steam is busy; try again in a minute".into());
    }
    if !resp.status().is_success() {
        return Err(format!("Steam said {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    parse(&v)
}

/// The summary's language: the reviews' one, English for "all".
fn i18n_api_language(language: &str) -> &str {
    if language == "all" { "english" } else { language }
}

/// The Web API's answer (`{"response": {...}}`). The summary's totals come
/// only with the first page; later pages leave them 0.
pub fn parse(v: &Value) -> Result<AppReviews, String> {
    let v = v.get("response").unwrap_or(v);
    if !v.is_object() || v.get("reviews").is_none() && v.get("query_summary").is_none() {
        return Err("Steam didn't return reviews for this game".into());
    }
    let s = &v["query_summary"];
    let num = |x: &Value| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
    let items = v["reviews"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| AppReview {
                    id: r["recommendationid"].as_str().map(String::from).unwrap_or_else(|| r["recommendationid"].to_string()),
                    author: num(&r["author"]["steamid"]),
                    minutes_played: num(&r["author"]["playtime_forever"]),
                    minutes_at_review: num(&r["author"]["playtime_at_review"]),
                    recommended: r["voted_up"].as_bool().unwrap_or(false),
                    text: r["review"].as_str().unwrap_or_default().trim().to_string(),
                    created: num(&r["timestamp_created"]),
                    helpful: num(&r["votes_up"]) as u32,
                    funny: num(&r["votes_funny"]) as u32,
                    early_access: r["written_during_early_access"].as_bool().unwrap_or(false),
                    free: r["received_for_free"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(AppReviews {
        label: s["review_score_desc"].as_str().unwrap_or_default().to_string(),
        positive: num(&s["total_positive"]) as u32,
        negative: num(&s["total_negative"]) as u32,
        total: num(&s["total_reviews"]) as u32,
        items,
        cursor: v["cursor"].as_str().map(String::from).filter(|c| !c.is_empty()),
    })
}

/// Your review of a game, if you wrote one: looked up on your profile's
/// Reviews pages (newest first; at most 30 pages are read).
pub async fn mine(web: &Web, app_id: AppId) -> Result<Option<UserReview>, String> {
    for page in 1..=30 {
        let list = crate::social::reviews(Some(web), web.me, page).await?;
        if let Some(r) = list.items.iter().find(|r| r.app_id == app_id) {
            return Ok(Some(r.clone()));
        }
        if list.items.is_empty() || page * 10 >= list.total {
            break;
        }
    }
    Ok(None)
}

/// What you're posting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub recommended: bool,
    pub public: bool,
    pub allow_comments: bool,
    /// Steam language name the review is written in.
    pub language: String,
}

/// Write or replace your review (Steam keeps one review per game and
/// account, so posting again edits it).
pub async fn post(web: &Web, app_id: AppId, d: &Draft) -> Result<(), String> {
    let app = app_id.to_string();
    let form = [
        ("sessionid", web.session_id.as_str()),
        ("appid", app.as_str()),
        ("steamworksappid", app.as_str()),
        ("comment", d.text.as_str()),
        ("rated_up", if d.recommended { "true" } else { "false" }),
        ("is_public", if d.public { "true" } else { "false" }),
        ("language", if d.language.is_empty() { "english" } else { d.language.as_str() }),
        ("received_compensation", "0"),
        ("disable_comments", if d.allow_comments { "0" } else { "1" }),
    ];
    let v: Value = web
        .http
        .post(format!("{STORE}/friends/recommendgame"))
        .header("Referer", format!("{STORE}/app/{app_id}/"))
        .form(&form)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| format!("Steam didn't accept the review ({e})"))?;
    check(&v)
}

/// Delete your review of a game.
pub async fn delete(web: &Web, app_id: AppId) -> Result<(), String> {
    let resp = web
        .http
        .post(format!("{COMMUNITY}/profiles/{}/recommended/", web.me))
        .header("Referer", format!("{COMMUNITY}/profiles/{}/recommended/{app_id}/", web.me))
        .form(&[("action", "delete"), ("sessionid", web.session_id.as_str()), ("appid", app_id.to_string().as_str())])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() { Ok(()) } else { Err(format!("Steam said {}", resp.status())) }
}

/// `{"success":true}` or Steam's reason.
fn check(v: &Value) -> Result<(), String> {
    if v["success"].as_bool() == Some(true) || v["success"].as_i64() == Some(1) {
        return Ok(());
    }
    let why = v["strError"].as_str().or_else(|| v["error"].as_str()).unwrap_or("Steam didn't accept the review");
    Err(why.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_steams_review_feed() {
        let v = serde_json::json!({"response": {
            "query_summary": {"review_score_desc": "Very Positive", "total_positive": 90, "total_negative": 10, "total_reviews": 100},
            "reviews": [{
                "recommendationid": "123",
                "author": {"steamid": "76561197960265729", "playtime_forever": 600, "playtime_at_review": 120},
                "review": " Great game \n", "timestamp_created": 1700000000, "voted_up": true,
                "votes_up": 5, "votes_funny": 1, "written_during_early_access": false, "received_for_free": true
            }],
            "cursor": "AoJ4"
        }});
        let r = super::parse(&v).unwrap();
        assert_eq!(r.label, "Very Positive");
        assert_eq!((r.positive, r.negative, r.total), (90, 10, 100));
        assert_eq!(r.items[0].author, 76561197960265729);
        assert_eq!(r.items[0].text, "Great game");
        assert!(r.items[0].recommended && r.items[0].free);
        assert_eq!(r.cursor.as_deref(), Some("AoJ4"));
        assert!(super::parse(&serde_json::json!({"response": {}})).is_err());
    }

    /// Live check against Steam: `cargo test -p client live_review_feed -- --ignored`.
    #[test]
    #[ignore]
    fn live_review_feed() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let first = rt.block_on(super::load(1145360, "english", "*")).unwrap();
        assert!(first.total > 1000 && !first.label.is_empty() && first.items.len() == 20, "{first:?}");
        assert!(first.items.iter().all(|r| r.author > 0 && !r.id.is_empty()));
        let next = rt.block_on(super::load(1145360, "english", first.cursor.as_deref().unwrap())).unwrap();
        assert_eq!(next.items.len(), 20);
        assert!(next.items.iter().all(|r| !first.items.iter().any(|f| f.id == r.id)), "second page repeats the first");
    }

    #[test]
    fn reads_post_results() {
        assert!(super::check(&serde_json::json!({"success": true})).is_ok());
        assert_eq!(super::check(&serde_json::json!({"success": false, "strError": "Too short"})), Err("Too short".to_string()));
    }

    #[test]
    fn builds_filtered_query() {
        use model::{ReviewFilter, ReviewKind, ReviewOrder, ReviewWindow};
        let f = ReviewFilter { kind: ReviewKind::Negative, order: ReviewOrder::Helpful, window: ReviewWindow::Week, all_languages: true };
        let q = super::request_input(570, "russian", "*", f);
        assert_eq!((q["filter"].as_i64(), q["day_range"].as_i64(), q["review_type"].as_i64()), (Some(0), Some(7), Some(2)));
        assert_eq!(q["languages"], serde_json::json!(["all"]));
        assert_eq!(q["display_language"], "english");

        let f2 = ReviewFilter { kind: ReviewKind::Positive, order: ReviewOrder::Funny, window: ReviewWindow::Month, all_languages: false };
        let q2 = super::request_input(570, "russian", "*", f2);
        assert_eq!((q2["filter"].as_i64(), q2["day_range"].as_i64(), q2["review_type"].as_i64()), (Some(3), Some(0), Some(1)));
        assert_eq!(q2["languages"], serde_json::json!(["russian"]));
        assert_eq!(q2["display_language"], "russian");
    }
}
