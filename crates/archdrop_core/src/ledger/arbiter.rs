use crate::ledger::models::Event;
use std::cmp::Ordering;

/// Compares two events according to Last-Write-Wins (LWW) arbitration rules:
/// 1. Higher `timestamp_ms` wins.
/// 2. If timestamps are equal, uppercase hex `origin_device_id` tie-breaker decides.
/// 3. If origin device IDs are equal, higher `sequence_number` wins.
pub fn compare_events_lww(a: &Event, b: &Event) -> Ordering {
    match a.timestamp_ms.cmp(&b.timestamp_ms) {
        Ordering::Equal => {
            let a_upper = a.origin_device_id.to_uppercase();
            let b_upper = b.origin_device_id.to_uppercase();
            match a_upper.cmp(&b_upper) {
                Ordering::Equal => a.sequence_number.cmp(&b.sequence_number),
                other => other,
            }
        }
        other => other,
    }
}

/// Deterministically resolves the single active event from a slice of candidate events
/// using Last-Write-Wins with device ID tie-breaker.
///
/// Note: Both events remain permanently in the ledger (zero dropped payloads);
/// this function arbitrates state for single-state consumers (e.g. system clipboard).
pub fn arbitrate_lww(events: &[Event]) -> Option<&Event> {
    events.iter().max_by(|a, b| compare_events_lww(a, b))
}

/// Filters for clipboard-related events (`event_type.starts_with("clipboard.")` or `clipboard.text` etc.)
/// and deterministically determines the winning active clipboard event.
pub fn arbitrate_clipboard(events: &[Event]) -> Option<&Event> {
    events
        .iter()
        .filter(|e| e.event_type.starts_with("clipboard."))
        .max_by(|a, b| compare_events_lww(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lww_higher_timestamp_wins() {
        let e1 = Event::new_local("DEVICE_A", 1, "clipboard.text", b"older".to_vec(), 1000);
        let e2 = Event::new_local("DEVICE_B", 1, "clipboard.text", b"newer".to_vec(), 2000);

        let candidates = [e1.clone(), e2.clone()];
        let winner = arbitrate_lww(&candidates).unwrap();
        assert_eq!(winner.payload_str().unwrap(), "newer");
    }

    #[test]
    fn test_lww_timestamp_tie_breaker_device_id() {
        // Same timestamp, DEVICE_F > DEVICE_E
        let e1 = Event::new_local("E8C2D3A1B4C5D6", 5, "clipboard.text", b"phone text".to_vec(), 1731062400000);
        let e2 = Event::new_local("F4A7B1C2D3E4F5", 5, "clipboard.text", b"laptop text".to_vec(), 1731062400000);

        let candidates = [e1.clone(), e2.clone()];
        let winner = arbitrate_lww(&candidates).unwrap();
        assert_eq!(winner.origin_device_id, "F4A7B1C2D3E4F5");
        assert_eq!(winner.payload_str().unwrap(), "laptop text");

        // Order of input does not affect deterministic winner
        let candidates_rev = [e2.clone(), e1.clone()];
        let winner_rev = arbitrate_lww(&candidates_rev).unwrap();
        assert_eq!(winner_rev.origin_device_id, "F4A7B1C2D3E4F5");
    }

    #[test]
    fn test_arbitrate_clipboard_filters_non_clipboard_events() {
        let e1 = Event::new_local("DEV_A", 1, "transfer.completed", b"file".to_vec(), 5000);
        let e2 = Event::new_local("DEV_B", 1, "clipboard.text", b"copied text".to_vec(), 3000);

        let candidates = [e1, e2];
        let winner = arbitrate_clipboard(&candidates).unwrap();
        assert_eq!(winner.payload_str().unwrap(), "copied text");
    }
}

