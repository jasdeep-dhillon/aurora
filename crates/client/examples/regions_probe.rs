//! `cargo run -p client --example regions_probe`: list download regions, fastest first.
#[tokio::main]
async fn main() {
    let list = client::regions::probe(local::find_steam_root()).await;
    println!("{} regions", list.len());
    for r in list.iter().take(15) {
        println!("  {:<20} cell {:>3}  ping {:>5?}  speed {:>7?} KB/s  load {:>3}  tier {}", r.name, r.cell_id, r.ping_ms, r.speed_kbps, r.load, r.speed_tier());
    }
}
