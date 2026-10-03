//! 同步接收的纯状态：无进展截止时间、会话隔离、封面分片完整性。
use std::time::{Duration, Instant};

pub const LIST_IDLE: Duration = Duration::from_secs(20);
pub const COVER_IDLE: Duration = Duration::from_secs(30);
pub const MAX_COVER_CHUNKS: usize = 65_536;

#[derive(Debug, Default, PartialEq)]
enum Phase {
    #[default]
    List,
    Covers,
}

#[derive(Debug, Default)]
pub struct SyncReceive {
    generation: u64,
    session: Option<String>,
    deadline: Option<Instant>,
    idle: Duration,
    phase: Phase,
}

impl SyncReceive {
    pub fn start(&mut self, now: Instant, session: Option<String>) {
        self.session = session;
        self.phase = Phase::List;
        self.enter(now, LIST_IDLE);
    }

    fn enter(&mut self, now: Instant, idle: Duration) {
        self.generation = self.generation.wrapping_add(1);
        self.idle = idle;
        self.deadline = Some(now + idle);
    }

    pub fn covers(&mut self, now: Instant) {
        if self.active() {
            self.phase = Phase::Covers;
            self.enter(now, COVER_IDLE);
        }
    }

    pub fn progress(&mut self, now: Instant) {
        if self.active() {
            self.deadline = Some(now + self.idle);
        }
    }

    /// HTTP 回调不通过业务定时器事件重启阶段，沿用当前看门狗的代际。
    pub fn http_covers(&mut self, now: Instant) {
        if self.active() {
            self.phase = Phase::Covers;
            self.idle = COVER_IDLE;
            self.progress(now);
        }
    }

    pub fn finish(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.deadline = None;
    }

    pub fn active(&self) -> bool {
        self.deadline.is_some()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn cover_phase(&self) -> bool {
        self.phase == Phase::Covers
    }

    pub fn remaining(&self, generation: u64, now: Instant) -> Option<Duration> {
        if generation != self.generation {
            return None;
        }
        self.deadline.map(|deadline| deadline.saturating_duration_since(now))
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub fn matches_session(&self, session: Option<&str>) -> bool {
        self.session.as_deref() == session
    }
}

#[derive(Debug)]
pub struct CoverChunks {
    pub total: usize,
    pieces: Vec<Option<String>>,
    received: usize,
}

impl CoverChunks {
    pub fn new(total: usize) -> Option<Self> {
        if total == 0 || total > MAX_COVER_CHUNKS {
            return None;
        }
        Some(Self { total, pieces: vec![None; total], received: 0 })
    }

    /// 重复片不计入有效进展；到齐一次性拼接。返回（整张封面，是否新片）。
    pub fn insert(&mut self, index: usize, total: usize, data: &str) -> (Option<String>, bool) {
        if total != self.total || index >= total || data.is_empty() {
            return (None, false);
        }
        let fresh = self.pieces[index].is_none();
        self.pieces[index] = Some(data.to_string());
        if fresh {
            self.received += 1;
        }
        if self.received != total {
            return (None, fresh);
        }
        let length = self.pieces.iter().map(|part| part.as_ref().unwrap().len()).sum();
        let mut cover = String::with_capacity(length);
        for part in &self.pieces {
            cover.push_str(part.as_ref().unwrap());
        }
        (Some(cover), fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_list_progress_can_exceed_twenty_seconds() {
        let now = Instant::now();
        let mut recv = SyncReceive::default();
        recv.start(now, None);
        let generation = recv.generation();
        for seconds in [19, 38, 57, 76] {
            recv.progress(now + Duration::from_secs(seconds));
        }
        assert_eq!(recv.remaining(generation, now + Duration::from_secs(80)), Some(Duration::from_secs(16)));
        assert_eq!(recv.remaining(generation, now + Duration::from_secs(96)), Some(Duration::ZERO));
    }

    #[test]
    fn phase_change_and_new_request_invalidate_old_timers() {
        let now = Instant::now();
        let mut recv = SyncReceive::default();
        recv.start(now, None);
        let list = recv.generation();
        recv.covers(now + Duration::from_secs(10));
        let covers = recv.generation();
        assert_eq!(recv.remaining(list, now + Duration::from_secs(100)), None);
        assert_eq!(recv.remaining(covers, now + Duration::from_secs(20)), Some(Duration::from_secs(20)));
        recv.start(now + Duration::from_secs(25), Some("new".into()));
        assert_eq!(recv.remaining(covers, now + Duration::from_secs(100)), None);
    }

    #[test]
    fn cover_progress_extends_idle_deadline_and_finish_stays_finished() {
        let now = Instant::now();
        let mut recv = SyncReceive::default();
        recv.start(now, None);
        recv.covers(now);
        let generation = recv.generation();
        for seconds in [29, 58, 87] {
            recv.progress(now + Duration::from_secs(seconds));
        }
        assert_eq!(recv.remaining(generation, now + Duration::from_secs(100)), Some(Duration::from_secs(17)));
        recv.finish();
        recv.progress(now + Duration::from_secs(101));
        assert!(!recv.active());
        assert_eq!(recv.remaining(generation, now + Duration::from_secs(120)), None);
    }

    #[test]
    fn negotiated_sessions_reject_old_frames_and_legacy_remains_compatible() {
        let now = Instant::now();
        let mut recv = SyncReceive::default();
        recv.start(now, Some("current".into()));
        assert!(recv.matches_session(Some("current")));
        assert!(!recv.matches_session(Some("old")));
        assert!(!recv.matches_session(None));
        recv.finish();
        assert!(recv.matches_session(Some("current"))); // 最终ACK丢失时仍能回复旧帧
        recv.start(now, None);
        assert!(recv.matches_session(None));
        assert!(!recv.matches_session(Some("old")));
    }

    #[test]
    fn reordered_cover_chunks_assemble_exactly_and_duplicates_are_not_progress() {
        let mut buf = CoverChunks::new(3).unwrap();
        assert_eq!(buf.insert(2, 3, "tail"), (None, true));
        assert_eq!(buf.insert(2, 3, "tail"), (None, false));
        assert_eq!(buf.insert(0, 3, "data:image/jpeg;base64,"), (None, true));
        assert_eq!(buf.insert(1, 3, "body"), (Some("data:image/jpeg;base64,bodytail".into()), true));
    }

    #[test]
    fn malformed_chunks_cannot_allocate_unbounded_slots_or_complete_a_cover() {
        assert!(CoverChunks::new(0).is_none());
        assert!(CoverChunks::new(MAX_COVER_CHUNKS + 1).is_none());
        let mut buf = CoverChunks::new(2).unwrap();
        assert_eq!(buf.insert(2, 2, "outside"), (None, false));
        assert_eq!(buf.insert(0, 3, "changed total"), (None, false));
        assert_eq!(buf.insert(0, 2, ""), (None, false));
        assert_eq!(buf.insert(1, 2, "tail"), (None, true));
        assert_eq!(buf.insert(0, 2, "head"), (Some("headtail".into()), true));
    }
}
