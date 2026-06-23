use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// Discord epoch: 2015-01-01T00:00:00Z
const DISCORD_EPOCH: u64 = 1_420_070_400_000;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn generate() -> u64 {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    let timestamp = ms - DISCORD_EPOCH;
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0xFFF;
    (timestamp << 22) | seq
}
