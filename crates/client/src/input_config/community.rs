use super::{BindingChoice, LayoutDocument, MAX_BYTES, store_layout};
use model::AppId;
use net::Connection;
use proto::*;
use std::sync::OnceLock;
use std::time::Duration;

const CREATOR_APP: u32 = 241100;
const CONTROLLER_FILE: u32 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutRequest {
    pub account: u32,
    pub app_id: AppId,
    pub request_id: u64,
}

#[derive(Debug, Clone)]
pub struct CommunityCursor {
    value: String,
    fallback: bool,
}

#[derive(Debug, Clone)]
pub struct CommunityLayout {
    pub id: u64,
    pub author: u64,
    pub title: String,
    pub description: String,
    pub controller_type: String,
    pub votes_up: u32,
    pub votes_down: u32,
    pub sessions: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CommunityPage {
    pub items: Vec<CommunityLayout>,
    pub next: Option<CommunityCursor>,
}

fn belongs_to(detail: &PublishedFileDetails, app_id: AppId) -> bool {
    detail.consumer_appid() == app_id || detail.kvtags.iter().any(|tag| {
        matches!(tag.key().to_ascii_lowercase().as_str(), "appid" | "app_id" | "gameid" | "game_id")
            && tag.value().parse::<u32>().ok() == Some(app_id)
    })
}

fn controller_file(detail: &PublishedFileDetails) -> bool {
    detail.file_type() == CONTROLLER_FILE && detail.creator_appid() == CREATOR_APP
        && !detail.banned() && detail.publishedfileid() != 0
}

pub(crate) async fn query(conn: &Connection, app_id: AppId, cursor: Option<CommunityCursor>) -> Result<CommunityPage, String> {
    let first = cursor.is_none();
    let mut fallback = cursor.as_ref().is_some_and(|c| c.fallback);
    let cursor = cursor.map_or_else(|| "*".into(), |c| c.value);
    let response = loop {
        let req = CPublishedFileQueryFilesRequest {
            query_type: Some(0),
            creator_appid: Some(CREATOR_APP),
            appid: (!fallback).then_some(app_id),
            filetype: Some(CONTROLLER_FILE),
            cursor: Some(cursor.clone()),
            numperpage: Some(30),
            required_kv_tags: if fallback {
                vec![c_published_file_query_files_request::KvTag {
                    key: Some("appid".into()), value: Some(app_id.to_string()),
                }]
            } else { Vec::new() },
            return_details: Some(true),
            return_tags: Some(true),
            return_kv_tags: Some(true),
            return_vote_data: Some(true),
            return_playtime_stats: Some(365),
            strip_description_bbcode: Some(true),
            ..Default::default()
        };
        let response = conn.call::<_, CPublishedFileQueryFilesResponse>("PublishedFile.QueryFiles#1", &req)
            .await.map_err(|err| err.to_string())?;
        if first && !fallback && !response.publishedfiledetails.iter().any(|d| controller_file(d) && belongs_to(d, app_id)) {
            fallback = true;
            continue;
        }
        break response;
    };
    let next = response.next_cursor.filter(|v| !v.is_empty() && v != &cursor && v.len() <= 4096)
        .map(|value| CommunityCursor { value, fallback });
    let items = response.publishedfiledetails.into_iter()
        .filter(|d| controller_file(d) && belongs_to(d, app_id)).take(30).map(|d| {
            let controller_type = d.kvtags.iter().find(|tag| tag.key().eq_ignore_ascii_case("controller_type"))
                .map(|tag| tag.value())
                .or_else(|| d.tags.iter().map(|tag| tag.tag()).find(|tag| tag.starts_with("controller_")))
                .unwrap_or_default().chars().take(120).collect();
            CommunityLayout {
                id: d.publishedfileid(), author: d.creator(),
                title: d.title().chars().take(256).collect(),
                description: d.file_description().chars().take(2048).collect(),
                controller_type,
                votes_up: d.vote_data.as_ref().map_or(0, |v| v.votes_up()),
                votes_down: d.vote_data.as_ref().map_or(0, |v| v.votes_down()),
                sessions: d.playtime_stats.as_ref().map_or_else(|| d.lifetime_playtime_sessions(), |v| v.num_sessions()),
            }
        }).collect();
    Ok(CommunityPage { items, next })
}

fn trusted(url: &reqwest::Url) -> bool {
    url.scheme() == "https" && url.port_or_known_default() == Some(443) && url.username().is_empty()
        && url.password().is_none() && url.host_str().is_some_and(|host| {
            matches!(host, "steamusercontent-a.akamaihd.net" | "steamuserimages-a.akamaihd.net" | "steamcdn-a.akamaihd.net")
            || host.ends_with(".steamusercontent.com") || host.ends_with(".steamstatic.com")
            || host.ends_with(".steamcontent.com")
        })
}

fn http() -> Result<&'static reqwest::Client, String> {
    static HTTP: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    HTTP.get_or_init(|| reqwest::Client::builder().https_only(true)
        .timeout(Duration::from_secs(20)).connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 || !trusted(attempt.url()) {
                attempt.error("untrusted Workshop redirect or redirect limit exceeded")
            } else {
                attempt.follow()
            }
        })).build()).as_ref().map_err(ToString::to_string)
}

pub(super) async fn download_text(conn: &Connection, id: u64, app_id: Option<AppId>) -> Result<String, String> {
    let req = CPublishedFileGetDetailsRequest {
        publishedfileids: vec![id],
        includekvtags: Some(true),
        ..Default::default()
    };
    let details = conn.call::<_, CPublishedFileGetDetailsResponse>("PublishedFile.GetDetails#1", &req)
        .await.map_err(|err| err.to_string())?;
    let detail = details.publishedfiledetails.iter().find(|d| {
        d.publishedfileid() == id && d.result() == 1 && !d.banned()
            && app_id.is_none_or(|app| controller_file(d) && belongs_to(d, app))
    }).ok_or("Steam did not return a matching controller layout")?;
    let raw = detail.file_url();
    let url = reqwest::Url::parse(raw).map_err(|_| "invalid Workshop download URL")?;
    if raw.contains('\\') || raw.chars().any(char::is_control) || !trusted(&url) {
        return Err("untrusted Workshop download URL".into());
    }
    let mut response = http()?.get(url).send().await.and_then(|r| r.error_for_status())
        .map_err(|err| err.without_url().to_string())?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err("Workshop download failed or exceeds 4 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| err.without_url().to_string())? {
        if chunk.len() > MAX_BYTES - bytes.len() {
            return Err("Workshop layout exceeds 4 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).map_err(|_| "Workshop layout is not UTF-8")?;
    LayoutDocument::parse(&text).map_err(|err| err.to_string())?;
    Ok(text)
}

pub(crate) async fn download(conn: &Connection, key: LayoutRequest, id: u64) -> Result<BindingChoice, String> {
    let text = download_text(conn, id, Some(key.app_id)).await?;
    let doc = LayoutDocument::parse(&text).map_err(|err| err.to_string())?;
    store_layout(key.account, key.app_id, "community", &format!("{id}.vdf"), &doc)?;
    Ok(BindingChoice::Community(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workshop_urls_are_https_on_steam_content_hosts() {
        for (url, expected) in [
            ("https://images.steamusercontent.com/ugc/1/file", true),
            ("https://steamusercontent-a.akamaihd.net/ugc/1/file", true),
            ("http://images.steamusercontent.com/ugc/1/file", false),
            ("https://images.steamusercontent.com.evil.example/", false),
            ("https://user@images.steamusercontent.com/", false),
            ("https://images.steamusercontent.com:444/", false),
        ] {
            assert_eq!(trusted(&reqwest::Url::parse(url).unwrap()), expected, "{url}");
        }
    }
}
