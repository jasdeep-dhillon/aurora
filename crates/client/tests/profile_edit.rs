use client::{Event, State};
use model::{ProfileEdit, ProfilePrivacy, ProfileSave};

#[test]
fn parses_edit_page_and_locations() {
    let body = r#"<div id="profile_edit_config" data-profile-edit="{&quot;strPersonaName&quot;:&quot;Pal &amp; Co&quot;,&quot;strCustomURL&quot;:&quot;pal&quot;,&quot;strRealName&quot;:&quot;Alex&quot;,&quot;strSummary&quot;:&quot;hi&quot;,&quot;LocationData&quot;:{&quot;locCountry&quot;:&quot;Kazakhstan&quot;,&quot;locCountryCode&quot;:&quot;KZ&quot;,&quot;locState&quot;:&quot;&quot;,&quot;locStateCode&quot;:&quot;&quot;,&quot;locCity&quot;:&quot;&quot;,&quot;locCityCode&quot;:0},&quot;Privacy&quot;:{&quot;PrivacySettings&quot;:{&quot;PrivacyProfile&quot;:3,&quot;PrivacyInventory&quot;:2,&quot;PrivacyInventoryGifts&quot;:1,&quot;PrivacyOwnedGames&quot;:3,&quot;PrivacyPlaytime&quot;:3,&quot;PrivacyFriendsList&quot;:3},&quot;eCommentPermission&quot;:0},&quot;ProfilePreferences&quot;:{&quot;hide_profile_awards&quot;:1},&quot;ActiveTheme&quot;:{&quot;theme_id&quot;:&quot;Midnight&quot;},&quot;rgAvailableThemes&quot;:[{&quot;theme_id&quot;:&quot;Midnight&quot;,&quot;title&quot;:&quot;Midnight&quot;}]}"></div>"#;
    let e = client::profile_edit::parse(body).unwrap();
    assert_eq!(e.persona_name, "Pal & Co");
    assert_eq!(e.real_name, "Alex");
    assert_eq!(e.custom_url, "pal");
    assert_eq!(e.summary, "hi");
    assert_eq!(e.country, "KZ");
    assert_eq!(e.location_text, "Kazakhstan");
    assert!(e.city.is_empty());
    assert!(e.hide_awards);
    assert_eq!(e.theme, "Midnight");
    assert_eq!(e.themes, vec![("Midnight".into(), "Midnight".into())]);
    assert_eq!(e.privacy, ProfilePrivacy { profile: 3, inventory: 2, gifts: 1, games: 3, playtime: 3, friends: 3, comments: 0 });

    let loc_json: serde_json::Value = serde_json::from_str(
        r#"[{"countrycode":"KZ","hasstates":1,"countryname":"Kazakhstan"},{"statecode":"02","statename":"Almaty"},{"cityid":1234,"cityname":"Almaty"}]"#,
    )
    .unwrap();
    assert_eq!(
        client::profile_edit::parse_locations(&loc_json),
        vec![("KZ".into(), "Kazakhstan".into()), ("02".into(), "Almaty".into()), ("1234".into(), "Almaty".into())]
    );
}

#[test]
fn state_tracks_profile_edit_and_locations() {
    let mut s = State::default();
    let edit = ProfileEdit { persona_name: "Tester".into(), country: "US".into(), ..Default::default() };
    s.apply(&Event::ProfileEdit(Ok(edit.clone())));
    assert_eq!(s.profile_edit.as_ref().unwrap().as_ref().unwrap(), &edit);

    s.apply(&Event::Locations {
        country: Some("US".into()),
        state: None,
        result: Ok(vec![("WA".into(), "Washington".into())]),
    });
    assert_eq!(
        s.locations.get(&(Some("US".into()), None)).unwrap(),
        &vec![("WA".into(), "Washington".into())]
    );

    // ProfileSaved leaves cached profile_edit intact until reloaded.
    s.apply(&Event::ProfileSaved { what: ProfileSave::Info, result: Ok(()) });
    assert!(s.profile_edit.is_some());
}
