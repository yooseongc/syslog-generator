use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::syslog_writer::{Facility, Severity, SyslogWriter};

/// TUI에서 실시간으로 변경 가능한 설정 (원자적 접근)
pub struct SharedConfig {
    pub rate: AtomicU64,       // 전체 목표 전송률 (0 = 최대 속도)
    pub msg_size: AtomicUsize, // 메시지 목표 크기 (bytes)
    pub paused: AtomicBool,
    pub num_threads: u64, // 불변 — rate 분배에 사용
}

impl SharedConfig {
    pub fn new(rate: u64, msg_size: usize, num_threads: u64) -> Self {
        Self {
            rate: AtomicU64::new(rate),
            msg_size: AtomicUsize::new(msg_size),
            paused: AtomicBool::new(false),
            num_threads,
        }
    }
}

/// 전송 통계 (원자적 접근, 여러 스레드에서 공유)
pub struct Stats {
    pub total_sent: AtomicU64,
    pub total_failed: AtomicU64,
    pub bytes_sent: AtomicU64,
    /// 최근 1초 평균 전송 지연 (microseconds)
    pub avg_latency_us: AtomicU64,
    /// 최근 1초 최소 전송 지연 (microseconds)
    pub min_latency_us: AtomicU64,
    /// 최근 1초 최대 전송 지연 (microseconds)
    pub max_latency_us: AtomicU64,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            total_sent: AtomicU64::new(0),
            total_failed: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            avg_latency_us: AtomicU64::new(0),
            min_latency_us: AtomicU64::new(u64::MAX),
            max_latency_us: AtomicU64::new(0),
        }
    }
}

/// 생성기 스레드마다 고정 설정
pub struct GeneratorConfig {
    pub facility: Facility,
    pub severity: Severity,
    pub prefix: String,
}

// ---------------------------------------------------------------------------
// 간단한 선형 합동 생성기 (LCG) — 빠른 의사난수, thread-local
// ---------------------------------------------------------------------------
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0xdeadbeef_cafebabe)
    }
    #[inline(always)]
    fn next(&mut self) -> u64 {
        // Knuth's multiplicative constants
        self.0 = self.0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
    /// 소문자 알파벳 한 글자
    #[inline(always)]
    fn next_char(&mut self) -> u8 {
        b'a' + (self.next() % 26) as u8
    }
}

// ---------------------------------------------------------------------------
// 랜덤 패딩 버퍼: msg_size 변경 시 재생성
// ---------------------------------------------------------------------------
struct PaddingBuf {
    buf: Vec<u8>,
    capacity: usize,
}

impl PaddingBuf {
    fn new(capacity: usize, lcg: &mut Lcg) -> Self {
        let buf = (0..capacity).map(|_| lcg.next_char()).collect();
        Self { buf, capacity }
    }

    /// 패딩 길이 변경 또는 다음 전송마다 일부 바이트를 갱신하여 메시지 다양성 확보
    fn resize_if_needed(&mut self, new_cap: usize, lcg: &mut Lcg) {
        if new_cap == self.capacity {
            return;
        }
        self.capacity = new_cap;
        self.buf.resize(new_cap, 0);
        for b in &mut self.buf {
            *b = lcg.next_char();
        }
    }

    /// 버퍼의 일부 바이트를 랜덤으로 갱신 (전체 재생성 대비 훨씬 빠름)
    fn refresh_partial(&mut self, lcg: &mut Lcg, n: usize) {
        let len = self.buf.len();
        if len == 0 { return; }
        for _ in 0..n {
            let idx = (lcg.next() % len as u64) as usize;
            self.buf[idx] = lcg.next_char();
        }
    }

    fn as_str(&self) -> &str {
        // SAFETY: buf는 항상 ASCII 소문자로만 채워짐
        unsafe { std::str::from_utf8_unchecked(&self.buf) }
    }
}

// ---------------------------------------------------------------------------
// 생성기 루프
// ---------------------------------------------------------------------------

/// syslog 생성 루프. running이 false가 되면 종료.
pub fn run_generator(
    config: GeneratorConfig,
    shared: Arc<SharedConfig>,
    stats: Arc<Stats>,
    running: Arc<AtomicBool>,
    thread_id: u64,
) {
    let writer = SyslogWriter::new(config.facility, config.severity);
    let base_prefix = format!("[syslog-gen] {}: ", config.prefix);

    // 스레드별로 다른 시드 (seq로 추가 변화)
    let mut lcg = Lcg::new(thread_id.wrapping_add(1) * 0x9e3779b97f4a7c15);

    // 초기 설정 로드
    let mut cur_rate = shared.rate.load(Ordering::Relaxed);
    let mut cur_size = shared.msg_size.load(Ordering::Relaxed);
    let mut interval = make_interval(cur_rate, shared.num_threads);

    // 패딩 버퍼 (seq= 부분을 제외한 나머지 공간)
    let pad_cap = pad_capacity(&base_prefix, cur_size);
    let mut pad_buf = PaddingBuf::new(pad_cap, &mut lcg);

    let mut seq: u64 = thread_id * 10_000_000; // 스레드별 seq 공간 분리
    let mut next_send = Instant::now();

    // 1초 윈도우 지역 통계
    let mut window_count: u64 = 0;
    let mut window_lat_sum: u64 = 0;
    let mut window_lat_min: u64 = u64::MAX;
    let mut window_lat_max: u64 = 0;
    let mut window_start = Instant::now();

    while running.load(Ordering::Acquire) {
        // pause 처리
        if shared.paused.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(20));
            window_start = Instant::now();
            window_count = 0;
            next_send = Instant::now();
            continue;
        }

        // 설정 변경 감지
        let new_rate = shared.rate.load(Ordering::Relaxed);
        if new_rate != cur_rate {
            cur_rate = new_rate;
            interval = make_interval(cur_rate, shared.num_threads);
            next_send = Instant::now();
        }
        let new_size = shared.msg_size.load(Ordering::Relaxed);
        if new_size != cur_size {
            cur_size = new_size;
            let new_cap = pad_capacity(&base_prefix, cur_size);
            pad_buf.resize_if_needed(new_cap, &mut lcg);
        }

        // rate 제어: 목표 시각까지 대기
        if let Some(iv) = interval {
            let now = Instant::now();
            if now < next_send {
                let remaining = next_send - now;
                if remaining > Duration::from_micros(500) {
                    std::thread::sleep(remaining - Duration::from_micros(200));
                }
                continue;
            }
            next_send += iv;
            // 100 주기 이상 지연 누적 시 리셋
            if next_send + iv * 100 < Instant::now() {
                next_send = Instant::now();
            }
        }

        // 전송마다 패딩 일부 갱신 → 메시지 다양성 확보
        // (syslog의 "message repeated" 압축 방지)
        pad_buf.refresh_partial(&mut lcg, 8);

        // 메시지 구성: seq=를 앞쪽에 배치하여 rsyslog 압축/잘림에도 고유성 보장
        let msg = format!("{}seq={:010} {}", base_prefix, seq, pad_buf.as_str());
        let msg_len = msg.len() as u64;

        // syslog 전송 + 지연 측정
        let t0 = Instant::now();
        let ok = writer.write(&msg);
        let latency_us = t0.elapsed().as_micros() as u64;

        if ok {
            stats.total_sent.fetch_add(1, Ordering::Relaxed);
            stats.bytes_sent.fetch_add(msg_len, Ordering::Relaxed);
        } else {
            stats.total_failed.fetch_add(1, Ordering::Relaxed);
        }

        seq += 1;
        window_count += 1;
        window_lat_sum += latency_us;
        if latency_us < window_lat_min { window_lat_min = latency_us; }
        if latency_us > window_lat_max { window_lat_max = latency_us; }

        // 1초마다 지연 통계를 공유 변수에 반영
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {

            if window_count > 0 {
                // 단순화: 마지막으로 쓰는 스레드 값이 최종 반영 (모니터링 근사치)
                stats.avg_latency_us.store(window_lat_sum / window_count, Ordering::Relaxed);
                // min: compare_exchange 루프로 경쟁 조건 없이 atomic 최솟값 갱신
                let mut cur = stats.min_latency_us.load(Ordering::Relaxed);
                while window_lat_min < cur {
                    match stats.min_latency_us.compare_exchange_weak(
                        cur, window_lat_min, Ordering::Relaxed, Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => cur = actual,
                    }
                }
                // max: compare_exchange 루프로 경쟁 조건 없이 atomic 최댓값 갱신
                let mut cur = stats.max_latency_us.load(Ordering::Relaxed);
                while window_lat_max > cur {
                    match stats.max_latency_us.compare_exchange_weak(
                        cur, window_lat_max, Ordering::Relaxed, Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => cur = actual,
                    }
                }
            }

            window_count = 0;
            window_lat_sum = 0;
            window_lat_min = u64::MAX;
            window_lat_max = 0;
            window_start = Instant::now();
        }
    }
}

fn make_interval(total_rate: u64, num_threads: u64) -> Option<Duration> {
    if total_rate == 0 {
        return None;
    }
    let thread_rate = (total_rate / num_threads.max(1)).max(1);
    let nanos = (1_000_000_000u64 / thread_rate).max(1); // 0이면 busy-loop 발생 → 최소 1ns
    Some(Duration::from_nanos(nanos))
}

/// 패딩 버퍼 크기 계산: 총 메시지 크기 - 헤더(prefix + "seq=NNNNNNNNNN ") 크기
fn pad_capacity(base_prefix: &str, msg_size: usize) -> usize {
    // "seq=0123456789 " = 15 chars
    let fixed_len = base_prefix.len() + 15;
    msg_size.saturating_sub(fixed_len)
}
