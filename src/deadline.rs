// deadline.rs — thread-safe host deadline and interrupt monitoring for Kaze VM.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DEADLINE_US: AtomicU64 = AtomicU64::new(0);

pub fn arm(ms: u64) {
    if ms == 0 {
        DEADLINE_US.store(0, Ordering::Relaxed);
        return;
    }
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    DEADLINE_US.store(now_us + ms * 1000, Ordering::Relaxed);
}

pub fn disarm() {
    DEADLINE_US.store(0, Ordering::Relaxed);
}

pub fn poll_expired() -> i32 {
    let dl = DEADLINE_US.load(Ordering::Relaxed);
    if dl == 0 {
        return 0;
    }
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    if now_us >= dl {
        1
    } else {
        0
    }
}

pub fn classify_interrupted(_s: &str) -> Option<String> {
    Some("E_TIMEOUT: the call was interrupted after its deadline elapsed".to_string())
}
