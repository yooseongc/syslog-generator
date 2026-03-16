use std::fs;
use std::time::Instant;

// ---------------------------------------------------------------------------
// CPU 스냅샷
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct CpuSnapshot {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuSnapshot {
    fn total(&self) -> u64 {
        self.user + self.nice + self.system + self.idle
            + self.iowait + self.irq + self.softirq + self.steal
    }
    fn busy(&self) -> u64 {
        self.total() - self.idle - self.iowait
    }
}

// ---------------------------------------------------------------------------
// 수집 결과 구조체
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SystemStats {
    // ── CPU
    pub cpu_usage_pct: f64,
    pub proc_cpu_pct: f64,

    // ── 메모리
    pub mem_total_kb: u64,
    pub mem_used_kb: u64,
    pub proc_vm_rss_kb: u64,

    // ── 프로세스 컨텍스트 스위치 (/proc/self/status, 초당)
    pub voluntary_ctxt_per_sec: u64,
    pub nonvoluntary_ctxt_per_sec: u64,

    // ── syslog 데몬 (rsyslogd / syslogd / syslog-ng)
    pub syslogd_name: String,
    pub syslogd_cpu_pct: f64,
    pub syslogd_rss_kb: u64,
    /// rsyslogd가 /dev/log 수신 후 로그 파일에 쓰는 속도 (/proc/PID/io write_bytes/sec)
    pub syslogd_write_bytes_per_sec: u64,
}

// ---------------------------------------------------------------------------
// Monitor
// ---------------------------------------------------------------------------

pub struct Monitor {
    ticks_per_sec: u64,

    // CPU
    prev_cpu: Option<CpuSnapshot>,
    prev_proc_ticks: Option<(u64, Instant)>,

    // syslog 데몬
    syslogd_pid: Option<u32>,
    syslogd_name: String,
    prev_syslogd_ticks: Option<(u64, Instant)>,
    syslogd_scan_at: Option<Instant>,               // 마지막 PID 스캔 시각
    prev_syslogd_io: Option<(u64, Instant)>,        // (write_bytes 누적, time)

    // 프로세스 컨텍스트 스위치
    prev_ctxt: Option<(u64, u64, Instant)>, // (voluntary, nonvoluntary, time)

}

impl Monitor {
    pub fn new() -> Self {
        let ticks_per_sec = {
            let r = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            if r <= 0 { 100 } else { r as u64 } // 오류(-1) 또는 비정상 값이면 기본값 100
        };
        Self {
            ticks_per_sec,
            prev_cpu: None,
            prev_proc_ticks: None,
            syslogd_pid: None,
            syslogd_name: String::new(),
            prev_syslogd_ticks: None,
            syslogd_scan_at: None,
            prev_syslogd_io: None,
            prev_ctxt: None,
        }
    }

    pub fn collect(&mut self) -> SystemStats {
        let cpu_usage_pct = self.read_cpu_usage();
        let proc_cpu_pct = self.read_proc_cpu(std::process::id());
        let (mem_total_kb, mem_used_kb) = read_mem_info();
        let proc_vm_rss_kb = read_rss(std::process::id());
        let (voluntary_ctxt_per_sec, nonvoluntary_ctxt_per_sec) = self.read_proc_ctxt();
        let (syslogd_cpu_pct, syslogd_rss_kb, syslogd_write_bytes_per_sec) = self.read_syslogd();
        let syslogd_name = self.syslogd_name.clone();

        SystemStats {
            cpu_usage_pct,
            proc_cpu_pct,
            mem_total_kb,
            mem_used_kb,
            proc_vm_rss_kb,
            voluntary_ctxt_per_sec,
            nonvoluntary_ctxt_per_sec,
            syslogd_name,
            syslogd_cpu_pct,
            syslogd_rss_kb,
            syslogd_write_bytes_per_sec,
        }
    }

    // ── 시스템 전체 CPU 사용률 ──────────────────────────────────────────

    fn read_cpu_usage(&mut self) -> f64 {
        let snap = read_cpu_snapshot();
        let result = if let Some(prev) = &self.prev_cpu {
            let total_delta = snap.total().saturating_sub(prev.total());
            let busy_delta = snap.busy().saturating_sub(prev.busy());
            if total_delta == 0 { 0.0 } else {
                (busy_delta as f64 / total_delta as f64) * 100.0
            }
        } else { 0.0 };
        self.prev_cpu = Some(snap);
        result
    }

    // ── 특정 프로세스 CPU 사용률 ──────────────────────────────────────

    fn read_proc_cpu(&mut self, pid: u32) -> f64 {
        let ticks = read_proc_ticks(pid);
        let now = Instant::now();
        let result = if let Some((prev_ticks, prev_time)) = self.prev_proc_ticks {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            let delta = ticks.saturating_sub(prev_ticks);
            if elapsed > 0.0 && self.ticks_per_sec > 0 {
                (delta as f64 / self.ticks_per_sec as f64 / elapsed) * 100.0
            } else { 0.0 }
        } else { 0.0 };
        self.prev_proc_ticks = Some((ticks, now));
        result
    }

    // ── 프로세스 컨텍스트 스위치 ──────────────────────────────────────

    fn read_proc_ctxt(&mut self) -> (u64, u64) {
        let (vol, nonvol) = parse_ctxt_switches(std::process::id());
        let now = Instant::now();
        let result = if let Some((prev_v, prev_nv, prev_t)) = self.prev_ctxt {
            let elapsed = now.duration_since(prev_t).as_secs_f64();
            if elapsed > 0.0 {
                // 1_000_000으로 상한 설정: elapsed가 매우 짧을 때 오버플로 방지
                let v_per_sec = (vol.saturating_sub(prev_v) as f64 / elapsed).min(1_000_000.0) as u64;
                let nv_per_sec = (nonvol.saturating_sub(prev_nv) as f64 / elapsed).min(1_000_000.0) as u64;
                (v_per_sec, nv_per_sec)
            } else { (0, 0) }
        } else { (0, 0) };
        self.prev_ctxt = Some((vol, nonvol, now));
        result
    }

    // ── syslog 데몬 모니터링 ──────────────────────────────────────────

    fn read_syslogd(&mut self) -> (f64, u64, u64) {
        // 10초마다 PID 재스캔
        let should_scan = self.syslogd_scan_at
            .map(|t| t.elapsed().as_secs() >= 10)
            .unwrap_or(true);

        if should_scan {
            if let Some((pid, name)) = find_syslogd() {
                self.syslogd_pid = Some(pid);
                self.syslogd_name = name;
            } else {
                self.syslogd_pid = None;
                self.syslogd_name = String::new();
            }
            self.syslogd_scan_at = Some(Instant::now());
        }

        let pid = match self.syslogd_pid {
            Some(p) => p,
            None => return (0.0, 0, 0),
        };

        let now = Instant::now();

        // CPU
        let ticks = read_proc_ticks(pid);
        let cpu_pct = if let Some((prev_ticks, prev_time)) = self.prev_syslogd_ticks {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            let delta = ticks.saturating_sub(prev_ticks);
            if elapsed > 0.0 && self.ticks_per_sec > 0 {
                (delta as f64 / self.ticks_per_sec as f64 / elapsed) * 100.0
            } else { 0.0 }
        } else { 0.0 };
        self.prev_syslogd_ticks = Some((ticks, now));

        // 디스크 쓰기: /proc/PID/io의 write_bytes (소켓 수신 후 로그 파일 기록)
        // write_bytes는 실제 파일시스템에 쓴 바이트 → 소켓 write와 달리 정확히 집계됨
        let (daemon_write_bytes, _) = parse_proc_io(pid);
        let write_per_sec = if let Some((prev_wb, prev_t)) = self.prev_syslogd_io {
            let elapsed = now.duration_since(prev_t).as_secs_f64();
            if elapsed > 0.0 {
                (daemon_write_bytes.saturating_sub(prev_wb) as f64 / elapsed) as u64
            } else { 0 }
        } else { 0 };
        self.prev_syslogd_io = Some((daemon_write_bytes, now));

        let rss = read_rss(pid);
        (cpu_pct, rss, write_per_sec)
    }
}

// ---------------------------------------------------------------------------
// 보조 함수들
// ---------------------------------------------------------------------------


fn read_cpu_snapshot() -> CpuSnapshot {
    let content = fs::read_to_string("/proc/stat").unwrap_or_default();
    let first = content.lines().next().unwrap_or("");
    let p: Vec<u64> = first.split_whitespace().skip(1)
        .filter_map(|v| v.parse().ok()).collect();
    CpuSnapshot {
        user:    p.get(0).copied().unwrap_or(0),
        nice:    p.get(1).copied().unwrap_or(0),
        system:  p.get(2).copied().unwrap_or(0),
        idle:    p.get(3).copied().unwrap_or(0),
        iowait:  p.get(4).copied().unwrap_or(0),
        irq:     p.get(5).copied().unwrap_or(0),
        softirq: p.get(6).copied().unwrap_or(0),
        steal:   p.get(7).copied().unwrap_or(0),
    }
}

fn read_mem_info() -> (u64, u64) {
    let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0u64;
    let mut free = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    for line in content.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "MemTotal:"  => total   = val,
            "MemFree:"   => free    = val,
            "Buffers:"   => buffers = val,
            "Cached:"    => cached  = val,
            _ => {}
        }
    }
    let used = total.saturating_sub(free).saturating_sub(buffers).saturating_sub(cached);
    (total, used)
}

fn read_rss(pid: u32) -> u64 {
    let path = format!("/proc/{}/status", pid);
    let content = fs::read_to_string(path).unwrap_or_default();
    for line in content.lines() {
        if line.starts_with("VmRSS:") {
            return line.split_whitespace().nth(1)
                .and_then(|v| v.parse().ok()).unwrap_or(0);
        }
    }
    0
}

fn read_proc_ticks(pid: u32) -> u64 {
    let path = format!("/proc/{}/stat", pid);
    let content = fs::read_to_string(path).unwrap_or_default();
    // /proc/PID/stat: comm 필드가 '(' ')' 로 감싸임. comm 자체에 ')'가 포함될 수 있으므로
    // 마지막 ')'의 위치를 찾아야 함 (rfind 사용)
    let after_comm = content.rfind(')').map(|i| &content[i + 1..]).unwrap_or("");
    let parts: Vec<&str> = after_comm.split_whitespace().collect();
    // utime=field 13, stime=field 14 (전체 기준). ')' 이후 기준으로 utime=11, stime=12
    let utime: u64 = parts.get(11).and_then(|v| v.parse().ok()).unwrap_or(0);
    let stime: u64 = parts.get(12).and_then(|v| v.parse().ok()).unwrap_or(0);
    utime + stime
}

fn parse_proc_io(pid: u32) -> (u64, u64) {
    let path = format!("/proc/{}/io", pid);
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut write_bytes = 0u64;
    let mut syscw = 0u64;
    for line in content.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "write_bytes:" => write_bytes = val,
            "syscw:"       => syscw       = val,
            _ => {}
        }
    }
    (write_bytes, syscw)
}

fn parse_ctxt_switches(pid: u32) -> (u64, u64) {
    let path = format!("/proc/{}/status", pid);
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut vol = 0u64;
    let mut nonvol = 0u64;
    for line in content.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "voluntary_ctxt_switches:"    => vol    = val,
            "nonvoluntary_ctxt_switches:" => nonvol = val,
            _ => {}
        }
    }
    (vol, nonvol)
}


/// /proc 디렉터리를 스캔하여 syslog 데몬 PID와 이름 반환
fn find_syslogd() -> Option<(u32, String)> {
    let targets = ["rsyslogd", "syslogd", "syslog-ng"];
    let dir = fs::read_dir("/proc").ok()?;
    for entry in dir.flatten() {
        let fname = entry.file_name();
        // to_str() 실패(비UTF-8 파일명)시 함수 전체를 종료하지 않고 해당 항목만 건너뜀
        let pid_str = match fname.to_str() {
            Some(s) => s,
            None => continue,
        };
        if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let comm_path = format!("/proc/{}/comm", pid_str);
        if let Ok(comm) = fs::read_to_string(&comm_path) {
            let comm = comm.trim();
            if targets.contains(&comm) {
                let pid: u32 = pid_str.parse().ok()?;
                return Some((pid, comm.to_string()));
            }
        }
    }
    None
}
