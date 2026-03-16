mod generator;
mod monitor;
mod syslog_writer;
mod tui;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::Parser;

use generator::{GeneratorConfig, SharedConfig, Stats};
use syslog_writer::{Facility, Severity};

#[derive(Parser, Debug)]
#[command(
    name = "syslog-gen",
    version = "1.0.0",
    about = "Linux syslog 부하 생성기 — TUI에서 실시간 rate/size 조정 가능"
)]
struct Cli {
    /// 초당 전송할 syslog 메시지 수 (0 = 최대 속도)
    #[arg(short = 'r', long, default_value = "100")]
    rate: u64,

    /// 로그 메시지 목표 크기 (bytes)
    #[arg(short = 's', long = "size", default_value = "200")]
    msg_size: usize,

    /// syslog facility (kern/user/daemon/auth/local0..local7 등)
    #[arg(short = 'f', long, default_value = "user")]
    facility: String,

    /// syslog severity (emerg/alert/crit/err/warn/notice/info/debug)
    #[arg(short = 'l', long = "level", default_value = "info")]
    severity: String,

    /// 메시지 접두사
    #[arg(short = 'p', long, default_value = "load-test")]
    prefix: String,

    /// TUI 없이 일정 시간(초)만 실행 후 종료 (0 = TUI 모드)
    #[arg(short = 't', long = "timeout", default_value = "0")]
    timeout_secs: u64,

    /// 생성기 스레드 수
    #[arg(short = 'n', long = "threads", default_value = "1")]
    threads: usize,
}

static RUNNING_PTR: AtomicUsize = AtomicUsize::new(0);

extern "C" fn sigint_handler(_: libc::c_int) {
    let ptr = RUNNING_PTR.load(Ordering::Relaxed);
    if ptr != 0 {
        // SAFETY: ptr은 main()의 `running` Arc 내부 포인터.
        // Arc는 main()이 살아있는 동안 유효 → 참조 카운트 변경 없이 직접 접근.
        unsafe { (*(ptr as *const AtomicBool)).store(false, Ordering::Release) };
    }
}

fn main() {
    let cli = Cli::parse();

    let facility: Facility = cli.facility.parse().unwrap_or_else(|e| {
        eprintln!("오류: {}", e);
        std::process::exit(1);
    });

    let severity: Severity = cli.severity.parse().unwrap_or_else(|e| {
        eprintln!("오류: {}", e);
        std::process::exit(1);
    });

    let threads = cli.threads.max(1);
    let stats = Arc::new(Stats::new());
    let shared = Arc::new(SharedConfig::new(cli.rate, cli.msg_size, threads as u64));
    let running = Arc::new(AtomicBool::new(true));

    // SIGINT 핸들러: Arc::as_ptr로 내부 포인터만 저장 (참조 카운트 변경 없음)
    RUNNING_PTR.store(Arc::as_ptr(&running) as usize, Ordering::Relaxed);
    unsafe {
        libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t);
    }

    // 생성기 스레드 시작
    let mut handles = Vec::new();
    for tid in 0..threads {
        let stats_clone = Arc::clone(&stats);
        let shared_clone = Arc::clone(&shared);
        let running_clone = Arc::clone(&running);
        let config = GeneratorConfig {
            facility,
            severity,
            prefix: cli.prefix.clone(),
        };
        handles.push(thread::spawn(move || {
            generator::run_generator(config, shared_clone, stats_clone, running_clone, tid as u64);
        }));
    }

    if cli.timeout_secs > 0 {
        // CLI 모드: 지정 시간 후 종료
        eprintln!(
            "rate={} logs/sec  size={} bytes  facility={}  severity={}  threads={}",
            cli.rate, cli.msg_size, facility, severity, threads
        );
        eprintln!("{}초 후 자동 종료...", cli.timeout_secs);
        thread::sleep(Duration::from_secs(cli.timeout_secs));
        running.store(false, Ordering::Release);
    } else {
        // TUI 모드
        let mut app = tui::TuiApp::new(
            Arc::clone(&stats),
            Arc::clone(&shared),
            facility.to_string(),
            severity.to_string(),
        );
        if let Err(e) = app.run() {
            eprintln!("TUI 오류: {}", e);
        }
        running.store(false, Ordering::Release);
    }

    for h in handles {
        let _ = h.join();
    }

    print_final_stats(&stats);
}

fn print_final_stats(stats: &Stats) {
    let total = stats.total_sent.load(Ordering::Relaxed);
    let failed = stats.total_failed.load(Ordering::Relaxed);
    let bytes = stats.bytes_sent.load(Ordering::Relaxed);
    let avg_lat = stats.avg_latency_us.load(Ordering::Relaxed);
    let min_lat = stats.min_latency_us.load(Ordering::Relaxed);
    let max_lat = stats.max_latency_us.load(Ordering::Relaxed);
    println!("=== 최종 통계 ===");
    println!("전송 완료:   {}", total);
    println!("실패:        {}", failed);
    println!("총 전송량:   {:.2} KB", bytes as f64 / 1024.0);
    println!("평균 지연:   {} μs", avg_lat);
    println!("최소 지연:   {} μs", if min_lat == u64::MAX { 0 } else { min_lat });
    println!("최대 지연:   {} μs", max_lat);
}
