//! Integration tests exercising `client::feed` parsing and status post
//! request/response helpers (runs cleanly even while `launch.rs` unit tests
//! have an unrelated Linux compile issue on origin/main).
//!
//! Ported from upstream SteamInRust (PR #101 & PR #107 by madonchik123).

use client::feed::{Post, PostKind, fill_from_details, parse_activity, parse_explore, status_answer, status_request};

const BASE_ID: u64 = 76561197960265728;

const ACTIVITY: &str = r##"<div class="blotter_day" id="blotter_day_1700000000">
	<div class="blotter_day_header">
		<div class="blotter_day_header_date">12 Sep</div>
		<div class="blotter_day_header_divider"></div>
	</div>
	<div class="blotter_block">
<div class="blotter_daily_rollup">
	<div class="blotter_daily_rollup_line">
		<div class="blotter_rollup_avatar">
			<div class="blotter_small_friend_block_container">
				<a href="https://steamcommunity.com/id/sample_one">
					<div class="blotter_playerAvatarSmall offline" >
						<img src="https://avatars.akamai.steamstatic.com/aaaa.jpg" data-miniprofile="1001">
					</div>
				</a>
			</div>
		</div>
		<span>
		<a href="https://steamcommunity.com/id/sample_one" data-miniprofile="1001">Sample One</a> and <a href="https://steamcommunity.com/id/sample_two" data-miniprofile="1002">Sample Two</a> are now friends.		</span>
	</div>
	<div class="blotter_daily_rollup_line">
		<div class="blotter_rollup_avatar"><img src="https://avatars.akamai.steamstatic.com/bbbb.jpg" data-miniprofile="1002"></div>
		<span>
		<a href="https://steamcommunity.com/profiles/76561197960266730" data-miniprofile="1002">Sample Two</a> achieved <a href="https://steamcommunity.com/app/620">Portal 2</a>:				</span>
		<img src="https://shared.akamai.steamstatic.com/community_assets/images/apps/620/ach.jpg" width="32" height="32"
title="Wake Up Call
Survive the manual override.">
	</div>
</div>
	</div>
	<div class="blotter_block">
<div class="blotter_workshopitempublished" data-appid="440" data-snr="x">
	<div class="blotter_author_block">
		<div class="blotter_avatar_holder">
			<a href="https://steamcommunity.com/id/sample_one">
				<div class="playerAvatar offline">
					<picture><img srcset="https://avatars.akamai.steamstatic.com/aaaa_medium.jpg" data-miniprofile="1001"></picture>
				</div>
			</a>
		</div>
		<div><a href="https://steamcommunity.com/id/sample_one" data-miniprofile="1001">Sample One</a></div>
		<div>published artwork for <a href="https://steamcommunity.com/app/440">Team Fortress 2</a></div>
	</div>
	<div class="blotter_workshopitem artwork">
		<div class="blotter_workshopitem_preview artwork">
			<a href="https://steamcommunity.com/sharedfiles/filedetails/?id=555" class="modalContentLink ugc" data-appid="440" data-publishedfileid="555">
				<img src="https://images.steamusercontent.com/ugc/1/ABC/?imw=320&amp;imh=181" />
			</a>
		</div>
		<div class="blotter_workshopitem_metadata">
			<div class="blotter_workshopitem_title"><a href="https://steamcommunity.com/sharedfiles/filedetails/?id=555">A drawing</a></div>
			<div class="blotter_workshopitem_description">My first one &gt;w&lt;</div>
		</div>
	</div>
	<div><div class="blotter_control_container"><span class="blotter_voters_names">8 people rated this up</span></div></div>
	<div class="blotter_comment_thread"><div class="blotter_viewallcomments_container"><a href="https://steamcommunity.com/sharedfiles/filedetails/?id=555"><span>View all 3 comments</span></a></div></div>
</div>
	</div>
	<div class="blotter_block">
<div class="blotter_userstatus" id="userstatus_1_2">
	<div class="blotter_author_block">
		<div class="blotter_avatar_holder"><a href="https://steamcommunity.com/id/sample_one"><div class="playerAvatar"><picture><img srcset="https://avatars.akamai.steamstatic.com/aaaa_medium.jpg" data-miniprofile="1001"></picture></div></a></div>
		<div><a href="https://steamcommunity.com/id/sample_one">Sample One</a></div>
		<div>posted a status about <a href="https://steamcommunity.com/app/440">Team Fortress 2</a></div>
	</div>
	<div class="blotter_userstatus_content responsive_body_text">meow</div>
</div>
	</div>
</div>"##;

#[test]
fn parses_friends_activity_json_and_blocks() {
    let body = serde_json::json!({
        "success": true,
        "blotter_html": ACTIVITY,
        "next_request": "https://steamcommunity.com/profiles/76561197960266729/ajaxgetusernews/?start=1699913600",
    })
    .to_string();
    let feed = parse_activity(&body).unwrap();
    assert_eq!(feed.next, 1699913600);
    let kinds: Vec<PostKind> = feed.posts.iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [PostKind::Status, PostKind::Achievement, PostKind::Artwork, PostKind::Status]);
    assert_eq!(feed.posts[2].title, "A drawing");
    assert_eq!(feed.posts[2].text, "My first one >w<");
    assert_eq!((feed.posts[2].votes_up, feed.posts[2].comments), (8, 3));
    assert_eq!(feed.posts[3].text, "meow");
}

#[test]
fn parses_explore_and_rejects_unsafe_modal_urls() {
    let html = r##"<div id="page1">
<div class="apphub_Card modalContentLink interactable ugc" data-modal-content-url="https://steamcommunity.com/sharedfiles/filedetails/?id=777" data-appid="440" data-publishedfileid="777">
	<div class="apphub_CardContentType">Team Fortress 2: Screenshot</div>
	<img class="apphub_CardContentPreviewImage" src="https://images.steamusercontent.com/ugc/2/DEF/?imw=1024">
	<div class="apphub_CardContentTitle">sunset</div>
</div>
<div class="apphub_Card modalContentLink interactable" data-modal-content-url="javascript:alert(1)" data-appid="440">
	<img class="apphub_CardContentPreviewImage" src="https://images.steamusercontent.com/ugc/3/XYZ/">
	<div class="apphub_CardContentTitle">bad</div>
</div>
</div>"##;
    let posts = parse_explore(html);
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].kind, PostKind::Screenshot);
    assert_eq!(posts[0].url, "https://steamcommunity.com/sharedfiles/filedetails/?id=777");
    assert_eq!(posts[1].url, "");
}

#[test]
fn fill_from_details_and_status_helpers_work() {
    let mut p = Post { kind: PostKind::Screenshot, file_id: 1, ..Default::default() };
    fill_from_details(&mut p, BASE_ID + 1, 3, "Ink sketch", 1_700_000_000);
    assert_eq!((p.kind, p.author, p.title.as_str(), p.time), (PostKind::Artwork, BASE_ID + 1, "Ink sketch", 1_700_000_000));

    let (url, form) = status_request(BASE_ID + 1, "abc", "  gg!  ", 570).unwrap();
    assert_eq!(url, "https://steamcommunity.com/profiles/76561197960265729/ajaxpostuserstatus/");
    assert_eq!(form[1].1, "gg!");
    assert_eq!(form[2].1, "570");
    assert!(status_request(BASE_ID + 1, "abc", "   ", 0).is_err());
    assert!(status_request(0, "abc", "hi", 0).is_err());
    assert_eq!(status_answer(r#"{"success":true}"#), Ok(()));
    assert_eq!(status_answer(r#"{"success":false,"message":"Too fast"}"#), Err("Too fast".into()));
}
