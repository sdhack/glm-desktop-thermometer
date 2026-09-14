#[test]
fn disk_temps_poll() {
    tempmon_sensor::spawn_disk_poller();
    std::thread::sleep(std::time::Duration::from_secs(5));
    let t = tempmon_sensor::disk_temps();
    println!("disk temps: {:?}", t);
    assert!(!t.is_empty(), "no disk temps read (native + fallback both failed)");
}
