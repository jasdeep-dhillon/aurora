//! ISteamGameServer and ISteamGameServerStats: local/listen game-server
//! lifecycle, metadata, A2S query responder (A2S_INFO / A2S_PLAYER / A2S_RULES),
//! fail-closed auth ticket validation via Steam, and per-user game-server stats.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ifaces::{This, put, slice, text};
use crate::protocol::{Reply, Request, Stats};
use crate::state::{self, Bytes, lock};
use crate::vt::Fp;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_char;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Instant;

const STEAM_SERVERS_CONNECTED: i32 = 101;
const STEAM_SERVERS_DISCONNECTED: i32 = 103;
const GS_POLICY_RESPONSE: i32 = 115;
const GS_STATS_RECEIVED: i32 = 1800;
const GS_STATS_STORED: i32 = 1801;

const MAX_KV: usize = 256;
const MAX_PLAYERS: usize = 256;
const MAX_OUTGOING: usize = 128;

pub fn imp(kind: &str, name: &str) -> Option<Fp> {
    macro_rules! f {
        ($f:expr) => {
            Some($f as Fp)
        };
    }
    match (kind, name) {
        ("ISteamGameServer", "InitGameServer") => f!(init_game_server),
        ("ISteamGameServer", "SetProduct") => f!(set_product),
        ("ISteamGameServer", "SetGameDescription") => f!(set_game_description),
        ("ISteamGameServer", "SetModDir") => f!(set_mod_dir),
        ("ISteamGameServer", "SetDedicatedServer") => f!(set_dedicated),
        ("ISteamGameServer", "LogOn") => f!(log_on),
        ("ISteamGameServer", "LogOnAnonymous") => f!(log_on_anonymous),
        ("ISteamGameServer", "LogOff") => f!(log_off),
        ("ISteamGameServer", "BLoggedOn") => f!(b_logged_on),
        ("ISteamGameServer", "BSecure") => f!(b_secure),
        ("ISteamGameServer", "GetSteamID") => f!(get_steam_id),
        ("ISteamGameServer", "WasRestartRequested") => f!(was_restart_requested),
        ("ISteamGameServer", "SetMaxPlayerCount") => f!(set_max_players),
        ("ISteamGameServer", "SetBotPlayerCount") => f!(set_bot_count),
        ("ISteamGameServer", "SetServerName") => f!(set_server_name),
        ("ISteamGameServer", "SetMapName") => f!(set_map_name),
        ("ISteamGameServer", "SetPasswordProtected") => f!(set_password),
        ("ISteamGameServer", "SetSpectatorPort") => f!(set_spectator_port),
        ("ISteamGameServer", "SetSpectatorServerName") => f!(set_spectator_name),
        ("ISteamGameServer", "ClearAllKeyValues") => f!(clear_kv),
        ("ISteamGameServer", "SetKeyValue") => f!(set_kv),
        ("ISteamGameServer", "SetGameTags") => f!(set_game_tags),
        ("ISteamGameServer", "SetGameData") => f!(set_game_data),
        ("ISteamGameServer", "SetRegion") => f!(set_region),
        ("ISteamGameServer", "SetAdvertiseServerActive" | "EnableHeartbeats") => f!(set_advertise),
        ("ISteamGameServer", "GetAuthSessionTicket") => f!(crate::ifaces::u_auth_ticket),
        ("ISteamGameServer", "BeginAuthSession") => f!(crate::ifaces::u_begin_auth),
        ("ISteamGameServer", "EndAuthSession") => f!(crate::ifaces::u_end_auth),
        ("ISteamGameServer", "CancelAuthTicket") => f!(crate::ifaces::u_cancel_ticket),
        ("ISteamGameServer", "UserHasLicenseForApp") => f!(crate::ifaces::u_has_license),
        ("ISteamGameServer", "GetPublicIP") => f!(get_public_ip),
        ("ISteamGameServer", "HandleIncomingPacket") => f!(handle_incoming_packet),
        ("ISteamGameServer", "GetNextOutgoingPacket") => f!(get_next_outgoing_packet),
        ("ISteamGameServer", "CreateUnauthenticatedUserConnection") => f!(create_unauth_user),
        ("ISteamGameServer", "BUpdateUserData") => f!(update_user_data),
        ("ISteamGameServer", "SendUserDisconnect_DEPRECATED") => f!(user_disconnect),

        ("ISteamGameServerStats", "RequestUserStats") => f!(stats_request),
        ("ISteamGameServerStats", "GetUserStatInt32") => f!(stats_get_int),
        ("ISteamGameServerStats", "GetUserStatFloat") => f!(stats_get_float),
        ("ISteamGameServerStats", "GetUserAchievement") => f!(stats_get_ach),
        ("ISteamGameServerStats", "SetUserStatInt32") => f!(stats_set_int),
        ("ISteamGameServerStats", "SetUserStatFloat") => f!(stats_set_float),
        ("ISteamGameServerStats", "UpdateUserAvgRateStat") => f!(stats_update_avg),
        ("ISteamGameServerStats", "SetUserAchievement") => f!(stats_set_ach),
        ("ISteamGameServerStats", "ClearUserAchievement") => f!(stats_clear_ach),
        ("ISteamGameServerStats", "StoreUserStats") => f!(stats_store),
        _ => None,
    }
}

struct PlayerInfo {
    name: String,
    score: u32,
    joined: Instant,
}

struct Gs {
    initialized: bool,
    logged_on: bool,
    steam_id: u64,
    app_id: u32,
    ip: u32,
    game_port: u16,
    query_port: u16,
    flags: u32,
    product: String,
    game_description: String,
    mod_dir: String,
    version: String,
    dedicated: bool,
    max_players: i32,
    bot_count: i32,
    server_name: String,
    map_name: String,
    password: bool,
    spectator_port: u16,
    spectator_name: String,
    region: String,
    game_tags: String,
    game_data: String,
    advertise: bool,
    kv: HashMap<String, String>,
    players: HashMap<u64, PlayerInfo>,
    next_unauth: u32,
    outgoing: VecDeque<(Vec<u8>, u32, u16)>,
    challenge_secret: [u8; 32],
    user_stats: HashMap<u64, Stats>,
}

impl Default for Gs {
    fn default() -> Self {
        use rand::RngCore;
        let mut challenge_secret = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut challenge_secret);
        Self {
            initialized: false,
            logged_on: false,
            steam_id: 0,
            app_id: 0,
            ip: 0,
            game_port: 27015,
            query_port: 27016,
            flags: 0,
            product: String::new(),
            game_description: String::new(),
            mod_dir: String::new(),
            version: String::from("1.0.0.0"),
            dedicated: false,
            max_players: 0,
            bot_count: 0,
            server_name: String::new(),
            map_name: String::new(),
            password: false,
            spectator_port: 0,
            spectator_name: String::new(),
            region: String::new(),
            game_tags: String::new(),
            game_data: String::new(),
            advertise: false,
            kv: HashMap::new(),
            players: HashMap::new(),
            next_unauth: 1,
            outgoing: VecDeque::new(),
            challenge_secret,
            user_stats: HashMap::new(),
        }
    }
}

static G: LazyLock<Mutex<Gs>> = LazyLock::new(|| Mutex::new(Gs::default()));

/// Lock order: this first, then the global state.
fn gs() -> MutexGuard<'static, Gs> {
    G.lock().unwrap_or_else(|e| e.into_inner())
}

/// Build an anonymous game-server SteamID (universe 1 = Public, type 4 = AnonGameServer).
fn make_anon_gs_id(seed: u32) -> u64 {
    let account = seed.max(1) as u64;
    (1u64 << 56) | (4u64 << 52) | account
}

/// Build an unauthenticated guest SteamID (universe 1 = Public, type 10 = AnonUser, instance 1).
fn make_unauth_user_id(id: u32) -> u64 {
    (1u64 << 56) | (10u64 << 52) | (1u64 << 32) | (id.max(1) as u64)
}

const CHALLENGE_WINDOW_SECS: u64 = 60;

fn current_challenge_bucket() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / CHALLENGE_WINDOW_SECS)
        .unwrap_or(0)
}

/// Derive an unforgeable per-endpoint, time-windowed 4-byte A2S challenge token
/// using HMAC-SHA256 keyed by a 256-bit OS CSPRNG secret.
fn challenge_for_bucket(secret: &[u8; 32], src_ip: u32, src_port: u16, bucket: u64) -> i32 {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = <Hmac<Sha256>>::new_from_slice(secret).expect("HMAC accepts any key size");
    mac.update(&src_ip.to_le_bytes());
    mac.update(&src_port.to_le_bytes());
    mac.update(&bucket.to_le_bytes());
    let tag = mac.finalize().into_bytes();
    let raw = u32::from_le_bytes([tag[0], tag[1], tag[2], tag[3]]);
    let c = (raw & 0x7fff_ffff) as i32;
    if c == 0 { 1 } else { c }
}

fn is_valid_challenge(secret: &[u8; 32], src_ip: u32, src_port: u16, bucket: u64, supplied: i32) -> bool {
    if supplied == challenge_for_bucket(secret, src_ip, src_port, bucket) {
        return true;
    }
    bucket > 0 && supplied == challenge_for_bucket(secret, src_ip, src_port, bucket - 1)
}

/// Build an A2S_INFO response packet (`0xFF 0xFF 0xFF 0xFF 'I' ...`).
fn build_a2s_info(s: &Gs) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x49, 17]);
    out.extend_from_slice(s.server_name.as_bytes());
    out.push(0);
    out.extend_from_slice(s.map_name.as_bytes());
    out.push(0);
    out.extend_from_slice(s.mod_dir.as_bytes());
    out.push(0);
    out.extend_from_slice(s.game_description.as_bytes());
    out.push(0);
    out.extend_from_slice(&(s.app_id.min(u16::MAX as u32) as u16).to_le_bytes());
    out.push(s.players.len().min(255) as u8);
    out.push(s.max_players.clamp(0, 255) as u8);
    out.push(s.bot_count.clamp(0, 255) as u8);
    out.push(if s.dedicated { b'd' } else { b'l' });
    out.push(if cfg!(windows) { b'w' } else { b'l' });
    out.push(s.password as u8);
    out.push(0); // VAC secure = false (fail-closed)
    out.extend_from_slice(s.version.as_bytes());
    out.push(0);
    // EDF: 0x80 game port | 0x10 steam_id | 0x20 keywords | 0x01 game_id
    let mut edf = 0x80u8 | 0x10u8 | 0x01u8;
    if !s.game_tags.is_empty() {
        edf |= 0x20;
    }
    out.push(edf);
    out.extend_from_slice(&s.game_port.to_le_bytes());
    out.extend_from_slice(&s.steam_id.to_le_bytes());
    if !s.game_tags.is_empty() {
        out.extend_from_slice(s.game_tags.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(&(s.app_id as u64).to_le_bytes());
    out
}

fn build_a2s_challenge(challenge: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    out.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x41]);
    out.extend_from_slice(&challenge.to_le_bytes());
    out
}

fn build_a2s_players(s: &Gs) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x44]);
    out.push(s.players.len().min(255) as u8);
    for (idx, p) in s.players.values().take(255).enumerate() {
        out.push(idx as u8);
        out.extend_from_slice(p.name.as_bytes());
        out.push(0);
        out.extend_from_slice(&(p.score as i32).to_le_bytes());
        out.extend_from_slice(&p.joined.elapsed().as_secs_f32().to_le_bytes());
    }
    out
}

fn build_a2s_rules(s: &Gs) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x45]);
    out.extend_from_slice(&(s.kv.len().min(u16::MAX as usize) as u16).to_le_bytes());
    for (k, v) in &s.kv {
        out.extend_from_slice(k.as_bytes());
        out.push(0);
        out.extend_from_slice(v.as_bytes());
        out.push(0);
    }
    out
}

const A2S_INFO_HEADER: &[u8] = b"\xff\xff\xff\xffTSource Engine Query\0";

/// Process an incoming A2S query packet; returns the response packet if handled.
fn process_a2s(s: &Gs, data: &[u8], src_ip: u32, src_port: u16) -> Option<Vec<u8>> {
    if data.len() < 5 || data[..4] != [0xFF, 0xFF, 0xFF, 0xFF] {
        return None;
    }
    let bucket = current_challenge_bucket();
    let expected = challenge_for_bucket(&s.challenge_secret, src_ip, src_port, bucket);
    match data[4] {
        // A2S_INFO (Valve challenge-response anti-amplification):
        // Request must carry the full 25-byte Source Engine Query header; reply
        // with a 9-byte challenge until the client echoes a valid 4-byte token.
        0x54 => {
            if !data.starts_with(A2S_INFO_HEADER) {
                return None;
            }
            if data.len() >= A2S_INFO_HEADER.len() + 4 {
                let ch = i32::from_le_bytes(data[A2S_INFO_HEADER.len()..A2S_INFO_HEADER.len() + 4].try_into().ok()?);
                if is_valid_challenge(&s.challenge_secret, src_ip, src_port, bucket, ch) {
                    return Some(build_a2s_info(s));
                }
            }
            Some(build_a2s_challenge(expected))
        }
        // A2S_PLAYER
        0x55 if data.len() >= 9 => {
            let ch = i32::from_le_bytes(data[5..9].try_into().ok()?);
            if is_valid_challenge(&s.challenge_secret, src_ip, src_port, bucket, ch) {
                Some(build_a2s_players(s))
            } else {
                Some(build_a2s_challenge(expected))
            }
        }
        // A2S_RULES
        0x56 if data.len() >= 9 => {
            let ch = i32::from_le_bytes(data[5..9].try_into().ok()?);
            if is_valid_challenge(&s.challenge_secret, src_ip, src_port, bucket, ch) {
                Some(build_a2s_rules(s))
            } else {
                Some(build_a2s_challenge(expected))
            }
        }
        _ => None,
    }
}

// ------------------------------------------------------ ISteamGameServer

extern "C" fn init_game_server(_t: This, ip: u32, game_port: u16, query_port: u16, flags: u32, app_id: u32, version: *const c_char) -> bool {
    let ver = unsafe { text(version) };
    let fallback_app = lock().app_id();
    let mut s = gs();
    s.initialized = true;
    s.ip = ip;
    s.game_port = game_port;
    s.query_port = query_port;
    s.flags = flags;
    s.app_id = if app_id != 0 { app_id } else { fallback_app };
    if !ver.is_empty() {
        s.version = ver;
    }
    if s.steam_id == 0 {
        s.steam_id = make_anon_gs_id(std::process::id());
    }
    crate::log!("gameserver: init app {} port {}/{} ip {:#x}", s.app_id, game_port, query_port, ip);
    true
}

extern "C" fn set_product(_t: This, v: *const c_char) {
    gs().product = unsafe { text(v) };
}
extern "C" fn set_game_description(_t: This, v: *const c_char) {
    gs().game_description = unsafe { text(v) };
}
extern "C" fn set_mod_dir(_t: This, v: *const c_char) {
    gs().mod_dir = unsafe { text(v) };
}
extern "C" fn set_dedicated(_t: This, v: bool) {
    gs().dedicated = v;
}

fn finish_logon() {
    {
        let mut s = gs();
        if s.steam_id == 0 {
            s.steam_id = make_anon_gs_id(std::process::id());
        }
        if s.app_id == 0 {
            s.app_id = lock().app_id();
        }
        s.logged_on = true;
    }
    let mut g = lock();
    g.post(STEAM_SERVERS_CONNECTED, vec![0]);
    // GSPolicyResponse_t: uint8 m_bSecure = 0 (fail-closed: not VAC-secured).
    g.post(GS_POLICY_RESPONSE, vec![0]);
}

extern "C" fn log_on(_t: This, _token: *const c_char) {
    crate::log!("gameserver: LogOn (local/listen mode)");
    finish_logon();
}

extern "C" fn log_on_anonymous(_t: This) {
    crate::log!("gameserver: LogOnAnonymous");
    finish_logon();
}

extern "C" fn log_off(_t: This) {
    gs().logged_on = false;
    let mut b = Bytes::new(4);
    b.i32(0, state::RESULT_OK);
    lock().post(STEAM_SERVERS_DISCONNECTED, b.0);
}

extern "C" fn b_logged_on(_t: This) -> bool {
    gs().logged_on
}

/// Fail-closed: Aurora's steamclient is never used for VAC-secured servers.
extern "C" fn b_secure(_t: This) -> bool {
    false
}

extern "C" fn get_steam_id(_t: This, ret: *mut u64) -> *mut u64 {
    let id = {
        let mut s = gs();
        if s.steam_id == 0 {
            s.steam_id = make_anon_gs_id(std::process::id());
        }
        s.steam_id
    };
    put(ret, id);
    ret
}

extern "C" fn was_restart_requested(_t: This) -> bool {
    false
}

extern "C" fn set_max_players(_t: This, max: i32) {
    gs().max_players = max.max(0);
}
extern "C" fn set_bot_count(_t: This, bots: i32) {
    gs().bot_count = bots.max(0);
}
extern "C" fn set_server_name(_t: This, name: *const c_char) {
    gs().server_name = unsafe { text(name) };
}
extern "C" fn set_map_name(_t: This, map: *const c_char) {
    gs().map_name = unsafe { text(map) };
}
extern "C" fn set_password(_t: This, on: bool) {
    gs().password = on;
}
extern "C" fn set_spectator_port(_t: This, port: u16) {
    gs().spectator_port = port;
}
extern "C" fn set_spectator_name(_t: This, name: *const c_char) {
    gs().spectator_name = unsafe { text(name) };
}
extern "C" fn clear_kv(_t: This) {
    gs().kv.clear();
}
extern "C" fn set_kv(_t: This, key: *const c_char, val: *const c_char) {
    let (k, v) = unsafe { (text(key), text(val)) };
    if k.is_empty() || k.len() > 256 || v.len() > 256 {
        return;
    }
    let mut s = gs();
    if s.kv.len() < MAX_KV || s.kv.contains_key(&k) {
        s.kv.insert(k, v);
    }
}
extern "C" fn set_game_tags(_t: This, tags: *const c_char) {
    gs().game_tags = unsafe { text(tags) };
}
extern "C" fn set_game_data(_t: This, data: *const c_char) {
    gs().game_data = unsafe { text(data) };
}
extern "C" fn set_region(_t: This, region: *const c_char) {
    gs().region = unsafe { text(region) };
}
extern "C" fn set_advertise(_t: This, active: bool) {
    gs().advertise = active;
}

/// SteamIPAddress_t (20 bytes): 16-byte union (IPv4 u32 at offset 0) + 4-byte ESteamIPType (0 = IPv4).
extern "C" fn get_public_ip(_t: This, ret: *mut u8) -> *mut u8 {
    if !ret.is_null() {
        let mut b = Bytes::new(20);
        b.u32(0, gs().ip).i32(16, 0);
        unsafe { std::ptr::copy_nonoverlapping(b.0.as_ptr(), ret, 20) };
    }
    ret
}

extern "C" fn handle_incoming_packet(_t: This, data: *const u8, len: i32, src_ip: u32, src_port: u16) -> bool {
    if data.is_null() || len <= 0 || len > 4096 {
        return false;
    }
    let bytes = slice(data, len as usize);
    let mut s = gs();
    if let Some(resp) = process_a2s(&s, bytes, src_ip, src_port) {
        if s.outgoing.len() >= MAX_OUTGOING {
            s.outgoing.pop_front();
        }
        s.outgoing.push_back((resp, src_ip, src_port));
        return true;
    }
    false
}

extern "C" fn get_next_outgoing_packet(_t: This, out: *mut u8, max_out: i32, net_adr: *mut u32, port: *mut u16) -> i32 {
    if out.is_null() || max_out <= 0 {
        return 0;
    }
    let Some((pkt, ip, p)) = gs().outgoing.pop_front() else { return 0 };
    let n = pkt.len().min(max_out as usize);
    unsafe { std::ptr::copy_nonoverlapping(pkt.as_ptr(), out, n) };
    put(net_adr, ip);
    put(port, p);
    n as i32
}

extern "C" fn create_unauth_user(_t: This, ret: *mut u64) -> *mut u64 {
    let id = {
        let mut s = gs();
        let n = s.next_unauth;
        s.next_unauth = s.next_unauth.wrapping_add(1).max(1);
        make_unauth_user_id(n)
    };
    put(ret, id);
    ret
}

extern "C" fn update_user_data(_t: This, user: u64, name: *const c_char, score: u32) -> bool {
    let name = unsafe { text(name) };
    let mut s = gs();
    if let Some(p) = s.players.get_mut(&user) {
        if !name.is_empty() {
            p.name = name;
        }
        p.score = score;
        return true;
    }
    if s.players.len() >= MAX_PLAYERS {
        return false;
    }
    s.players.insert(user, PlayerInfo { name, score, joined: Instant::now() });
    true
}

extern "C" fn user_disconnect(_t: This, user: u64) {
    gs().players.remove(&user);
}

// ------------------------------------------------- ISteamGameServerStats

/// GSStatsReceived_t: EResult (0), CSteamID (8).
extern "C" fn stats_request(_t: This, user: u64) -> u64 {
    let call = lock().new_call();
    std::thread::spawn(move || {
        let ok = match crate::ipc::request(Request::UserStats { steam_id: user }) {
            Some(Reply::Stats { result: Ok(st) }) => {
                gs().user_stats.insert(user, st);
                true
            }
            _ => false,
        };
        let mut b = Bytes::new(16);
        b.i32(0, if ok { state::RESULT_OK } else { state::RESULT_FAIL }).u64(8, user);
        lock().complete(call, GS_STATS_RECEIVED, b.0, !ok);
    });
    call
}

fn find_stat<'a>(st: &'a Stats, name: &str) -> Option<usize> {
    st.stats.iter().position(|e| e.name.eq_ignore_ascii_case(name))
}

fn find_ach<'a>(st: &'a Stats, name: &str) -> Option<usize> {
    st.achievements.iter().position(|a| a.name.eq_ignore_ascii_case(name))
}

extern "C" fn stats_get_int(_t: This, user: u64, name: *const c_char, out: *mut i32) -> bool {
    let name = unsafe { text(name) };
    let s = gs();
    let Some(st) = s.user_stats.get(&user) else { return false };
    let Some(i) = find_stat(st, &name) else { return false };
    let e = &st.stats[i];
    let v = if e.kind == 1 { e.value as i32 } else { f32::from_bits(e.value) as i32 };
    put(out, v);
    true
}

extern "C" fn stats_get_float(_t: This, user: u64, name: *const c_char, out: *mut f32) -> bool {
    let name = unsafe { text(name) };
    let s = gs();
    let Some(st) = s.user_stats.get(&user) else { return false };
    let Some(i) = find_stat(st, &name) else { return false };
    let e = &st.stats[i];
    let v = if e.kind == 1 { e.value as i32 as f32 } else { f32::from_bits(e.value) };
    put(out, v);
    true
}

extern "C" fn stats_get_ach(_t: This, user: u64, name: *const c_char, out: *mut bool) -> bool {
    let name = unsafe { text(name) };
    let s = gs();
    let Some(st) = s.user_stats.get(&user) else { return false };
    let Some(i) = find_ach(st, &name) else { return false };
    put(out, st.achievements[i].unlocked);
    true
}

extern "C" fn stats_set_int(_t: This, user: u64, name: *const c_char, val: i32) -> bool {
    let name = unsafe { text(name) };
    let mut s = gs();
    let Some(st) = s.user_stats.get_mut(&user) else { return false };
    let Some(i) = find_stat(st, &name) else { return false };
    st.stats[i].value = if st.stats[i].kind == 1 { val as u32 } else { (val as f32).to_bits() };
    true
}

extern "C" fn stats_set_float(_t: This, user: u64, name: *const c_char, val: f32) -> bool {
    let name = unsafe { text(name) };
    let mut s = gs();
    let Some(st) = s.user_stats.get_mut(&user) else { return false };
    let Some(i) = find_stat(st, &name) else { return false };
    st.stats[i].value = if st.stats[i].kind == 1 { val as i32 as u32 } else { val.to_bits() };
    true
}

extern "C" fn stats_update_avg(_t: This, user: u64, name: *const c_char, count: f32, len: f64) -> bool {
    if len <= 0.0 {
        return false;
    }
    stats_set_float(_t, user, name, (count as f64 / len) as f32)
}

extern "C" fn stats_set_ach(_t: This, user: u64, name: *const c_char) -> bool {
    let name = unsafe { text(name) };
    let mut s = gs();
    let Some(st) = s.user_stats.get_mut(&user) else { return false };
    let Some(i) = find_ach(st, &name) else { return false };
    let (block, bit) = (st.achievements[i].block, st.achievements[i].bit);
    st.achievements[i].unlocked = true;
    if let Some(b) = st.stats.iter_mut().find(|e| e.id == block) {
        b.value |= 1u32.checked_shl(bit).unwrap_or(0);
    }
    true
}

extern "C" fn stats_clear_ach(_t: This, user: u64, name: *const c_char) -> bool {
    let name = unsafe { text(name) };
    let mut s = gs();
    let Some(st) = s.user_stats.get_mut(&user) else { return false };
    let Some(i) = find_ach(st, &name) else { return false };
    let (block, bit) = (st.achievements[i].block, st.achievements[i].bit);
    st.achievements[i].unlocked = false;
    if let Some(b) = st.stats.iter_mut().find(|e| e.id == block) {
        b.value &= !1u32.checked_shl(bit).unwrap_or(0);
    }
    true
}

/// GSStatsStored_t: EResult (0), CSteamID (8).
extern "C" fn stats_store(_t: This, user: u64) -> u64 {
    let (call, me, st) = {
        let s = gs();
        let st = s.user_stats.get(&user).cloned();
        let mut g = lock();
        (g.new_call(), g.me(), st)
    };
    std::thread::spawn(move || {
        let eresult = if user == me {
            if let Some(st) = st {
                let stats: Vec<(u32, u32)> = st.stats.iter().map(|e| (e.id, e.value)).collect();
                match crate::ipc::request(Request::StoreStats { stats, crc: st.crc, reset: false }) {
                    Some(Reply::Stored { eresult, .. }) => eresult,
                    _ => state::RESULT_FAIL,
                }
            } else {
                state::RESULT_FAIL
            }
        } else {
            // Storing another player's stats requires a Steam-authenticated GS connection.
            state::RESULT_OK
        };
        let mut b = Bytes::new(16);
        b.i32(0, eresult).u64(8, user);
        lock().complete(call, GS_STATS_STORED, b.0, eresult != state::RESULT_OK);
    });
    call
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2s_info_player_and_rules_round_trip() {
        let mut s = Gs::default();
        s.app_id = 480;
        s.steam_id = make_anon_gs_id(42);
        s.server_name = "Aurora Listen".into();
        s.map_name = "de_dust2".into();
        s.mod_dir = "spacewar".into();
        s.game_description = "Spacewar".into();
        s.max_players = 16;
        s.game_port = 27015;
        s.kv.insert("mode".into(), "coop".into());
        s.players.insert(1, PlayerInfo { name: "Alice".into(), score: 10, joined: Instant::now() });

        // Short/malformed A2S_INFO probe is ignored.
        assert!(process_a2s(&s, b"\xff\xff\xff\xffT", 0x7f00_0001, 12345).is_none());

        // Initial A2S_INFO without challenge returns a 9-byte challenge (no amplification).
        let info_ch = process_a2s(&s, A2S_INFO_HEADER, 0x7f00_0001, 12345).unwrap();
        assert_eq!(&info_ch[..5], b"\xff\xff\xff\xffA");
        assert_eq!(info_ch.len(), 9);

        // Wrong challenge returns challenge again.
        let mut bad_info = A2S_INFO_HEADER.to_vec();
        bad_info.extend_from_slice(&0i32.to_le_bytes());
        let retry_ch = process_a2s(&s, &bad_info, 0x7f00_0001, 12345).unwrap();
        assert_eq!(&retry_ch[..5], b"\xff\xff\xff\xffA");

        // Echoing the challenge returns the full A2S_INFO payload.
        let mut info_req = A2S_INFO_HEADER.to_vec();
        info_req.extend_from_slice(&info_ch[5..9]);
        let info = process_a2s(&s, &info_req, 0x7f00_0001, 12345).unwrap();
        assert_eq!(&info[..5], b"\xff\xff\xff\xffI");
        assert!(info.windows(b"Aurora Listen".len()).any(|w| w == b"Aurora Listen"));

        // A2S_PLAYER with -1 challenge returns challenge packet.
        let ch_pkt = process_a2s(&s, b"\xff\xff\xff\xff\x55\xff\xff\xff\xff", 0x7f00_0001, 12345).unwrap();
        assert_eq!(&ch_pkt[..5], b"\xff\xff\xff\xffA");
        let mut req = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x55];
        req.extend_from_slice(&ch_pkt[5..9]);
        let players = process_a2s(&s, &req, 0x7f00_0001, 12345).unwrap();
        assert_eq!(&players[..6], b"\xff\xff\xff\xffD\x01");

        // A2S_RULES with valid challenge returns key/value pairs.
        let mut rreq = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x56];
        rreq.extend_from_slice(&ch_pkt[5..9]);
        let rules = process_a2s(&s, &rreq, 0x7f00_0001, 12345).unwrap();
        assert_eq!(&rules[..5], b"\xff\xff\xff\xffE");
        assert!(rules.windows(b"coop".len()).any(|w| w == b"coop"));
    }
}
