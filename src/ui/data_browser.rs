//! Device library presentation state, independent of HTTP/interconnect transport.
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDevice {
    pub name: String,
    pub addr: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum DataPhase {
    #[default]
    Idle,
    Connecting,
    Lists,
    Covers,
    Finished,
    Failed,
}

#[derive(Debug, Default)]
pub struct DataBrowser {
    pub revision: u64,
    pub owner: Option<DataDevice>,
    pub requested_device: Option<DataDevice>,
    pub checked_device: Option<DataDevice>,
    pub last_complete: Option<(DataDevice, u64)>,
    pub phase: DataPhase,
    pub incoming: bool,
    pub lists_done: bool,
    pub lists_complete: bool,
    pub covers_received: usize,
    pub covers_skipped: Option<usize>,
    pub error: Option<String>,
}

impl DataBrowser {
    pub fn busy(&self) -> bool {
        matches!(self.phase, DataPhase::Connecting | DataPhase::Lists | DataPhase::Covers)
    }

    pub fn begin(&mut self, device: DataDevice) {
        self.begin_request(Some(device));
    }

    pub fn begin_request(&mut self, device: Option<DataDevice>) {
        self.revision = self.revision.wrapping_add(1);
        self.requested_device = device;
        self.phase = DataPhase::Connecting;
        self.incoming = false;
        self.error = None;
    }

    /// Switch the visible snapshot only on the first accepted data message.
    pub fn accept_data(&mut self) -> bool {
        if self.incoming { return false; }
        self.incoming = true;
        self.owner = self.requested_device.clone();
        self.lists_done = false;
        self.lists_complete = false;
        self.covers_received = 0;
        self.covers_skipped = None;
        self.phase = DataPhase::Lists;
        true
    }

    pub fn finish(&mut self, now: u64, covers_received: usize, skipped: Option<usize>) {
        self.covers_received = covers_received;
        self.covers_skipped = skipped;
        self.phase = DataPhase::Finished;
        if self.lists_complete {
            if let Some(owner) = &self.owner { self.last_complete = Some((owner.clone(), now)); }
        }
    }

    pub fn fail(&mut self, message: String) {
        self.phase = DataPhase::Failed;
        self.error = Some(message);
    }

    pub fn owner_matches_connection(&self) -> bool {
        self.owner.as_ref().zip(self.checked_device.as_ref())
            .is_some_and(|(owner, device)| owner.addr == device.addr)
    }

    pub fn complete_time(&self) -> Option<u64> {
        self.owner.as_ref().zip(self.last_complete.as_ref())
            .filter(|(owner, (device, _))| owner.addr == device.addr)
            .map(|(_, (_, time))| *time)
    }
}

pub fn matching_indices<'a>(names: impl Iterator<Item = &'a str>, query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    names.enumerate().filter_map(|(index, name)| {
        (!name.is_empty() && (query.is_empty() || name.to_lowercase().contains(&query))).then_some(index)
    }).collect()
}

pub fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_secs()).unwrap_or(0)
}

/// WASI does not expose the host's local timezone; label the displayed clock UTC.
pub fn format_sync_time(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        seconds / 3600 % 24, seconds / 60 % 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(addr: &str) -> DataDevice { DataDevice { name: addr.into(), addr: addr.into() } }

    #[test]
    fn search_keeps_original_slots_and_handles_case_whitespace_and_empty_slots() {
        let names = ["", "漫画 A", "Book One", "BOOK Two", "漫画 B"];
        assert_eq!(matching_indices(names.iter().copied(), " book "), vec![2, 3]);
        assert_eq!(matching_indices(names.iter().copied(), "漫画"), vec![1, 4]);
        assert_eq!(matching_indices(names.iter().copied(), "  "), vec![1, 2, 3, 4]);
        assert!(matching_indices(names.iter().copied(), "absent").is_empty());
    }

    #[test]
    fn failed_refresh_preserves_owner_and_complete_time_until_new_data_arrives() {
        let mut data = DataBrowser::default();
        data.begin(device("A"));
        assert!(data.accept_data());
        data.lists_complete = true;
        data.finish(100, 0, Some(1));
        data.begin(device("B"));
        data.fail("no reply".into());
        assert_eq!(data.owner.as_ref().unwrap().addr, "A");
        assert_eq!(data.complete_time(), Some(100));
        data.begin(device("B"));
        assert!(data.accept_data());
        assert!(!data.accept_data());
        assert_eq!(data.owner.as_ref().unwrap().addr, "B");
        assert_eq!(data.complete_time(), None);
        data.finish(200, 0, None);
        assert_eq!(data.complete_time(), None, "partial lists cannot advance full-sync time");
    }

    #[test]
    fn connection_and_request_do_not_relabel_the_displayed_snapshot() {
        let mut data = DataBrowser::default();
        data.begin(device("A"));
        data.accept_data();
        let revision = data.revision;
        data.checked_device = Some(device("B"));
        assert!(!data.owner_matches_connection());
        data.begin(device("B"));
        assert_ne!(data.revision, revision);
        assert_eq!(data.owner.as_ref().unwrap().addr, "A");
    }

    #[test]
    fn utc_time_handles_epoch_leap_day_and_day_boundary() {
        assert_eq!(format_sync_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_sync_time(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(format_sync_time(86_399), "1970-01-01 23:59:59 UTC");
    }
}
