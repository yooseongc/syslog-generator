use std::collections::VecDeque;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame, Terminal,
};

use crate::generator::{SharedConfig, Stats};
use crate::monitor::{Monitor, SystemStats};

pub struct TuiApp {
    stats: Arc<Stats>,
    shared: Arc<SharedConfig>,
    facility_name: String,
    severity_name: String,
    history: VecDeque<f64>,  // 샘플링된 전송률 (logs/sec)
    sys_stats: SystemStats,
    monitor: Monitor,
    // 전송률/바이트 계산용: total_sent, bytes_sent 델타
    prev_total_sent: u64,
    prev_bytes_sent: u64,
    prev_sample_time: Instant,
    /// 최근 샘플 전송 바이트/sec
    bytes_rate: u64,
}

impl TuiApp {
    pub fn new(
        stats: Arc<Stats>,
        shared: Arc<SharedConfig>,
        facility_name: String,
        severity_name: String,
    ) -> Self {
        Self {
            stats,
            shared,
            facility_name,
            severity_name,
            history: VecDeque::with_capacity(120),
            sys_stats: SystemStats::default(),
            monitor: Monitor::new(),
            prev_total_sent: 0,
            prev_bytes_sent: 0,
            prev_sample_time: Instant::now(),
            bytes_rate: 0,
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        loop {
            // 전송률/바이트: total_sent, bytes_sent 델타 / 경과 시간
            let now = Instant::now();
            let total   = self.stats.total_sent.load(Ordering::Relaxed);
            let bytes   = self.stats.bytes_sent.load(Ordering::Relaxed);
            let elapsed_secs = now.duration_since(self.prev_sample_time).as_secs_f64();
            let (rate, bytes_rate) = if elapsed_secs > 0.01 {
                let r = (total.saturating_sub(self.prev_total_sent) as f64 / elapsed_secs) as u64;
                let b = (bytes.saturating_sub(self.prev_bytes_sent) as f64 / elapsed_secs) as u64;
                (r, b)
            } else {
                (self.history.back().copied().unwrap_or(0.0) as u64, self.bytes_rate)
            };
            self.prev_total_sent = total;
            self.prev_bytes_sent = bytes;
            self.prev_sample_time = now;
            self.bytes_rate = bytes_rate;

            self.history.push_back(rate as f64);
            if self.history.len() > 120 {
                self.history.pop_front();
            }

            // 시스템 통계 수집
            self.sys_stats = self.monitor.collect();

            terminal.draw(|f| self.render(f))?;

            // 키 이벤트 처리 (200ms 타임아웃)
            if event::poll(Duration::from_millis(200))? {
                if let Event::Key(key) = event::read()? {
                    // Press 이벤트만 처리 (Release/Repeat 무시)
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match (key.code, key.modifiers) {
                        // 종료
                        (KeyCode::Char('q'), _)
                        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,

                        // rate 조절: ↑↓ ±100, ←→ ±1000
                        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                            self.adjust_rate(100);
                        }
                        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                            self.adjust_rate(-100);
                        }
                        (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
                            self.adjust_rate(1000);
                        }
                        (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
                            self.adjust_rate(-1000);
                        }
                        // rate 10배/1/10
                        (KeyCode::PageUp, _) => {
                            let cur = self.shared.rate.load(Ordering::Relaxed);
                            self.shared.rate.store((cur * 10).min(10_000_000), Ordering::Relaxed);
                        }
                        (KeyCode::PageDown, _) => {
                            let cur = self.shared.rate.load(Ordering::Relaxed);
                            self.shared.rate.store((cur / 10).max(1), Ordering::Relaxed);
                        }

                        // 메시지 크기: [/] ±64, {/} ±512
                        (KeyCode::Char(']'), _) => self.adjust_size(64),
                        (KeyCode::Char('['), _) => self.adjust_size(-64),
                        (KeyCode::Char('}'), _) => self.adjust_size(512),
                        (KeyCode::Char('{'), _) => self.adjust_size(-512),

                        // 일시정지/재개
                        (KeyCode::Char(' '), _) => {
                            let p = self.shared.paused.load(Ordering::Relaxed);
                            self.shared.paused.store(!p, Ordering::Relaxed);
                            // pause 중에는 latency 통계 리셋
                            if !p {
                                self.stats.min_latency_us.store(u64::MAX, Ordering::Relaxed);
                                self.stats.max_latency_us.store(0, Ordering::Relaxed);
                            }
                        }

                        _ => {}
                    }
                }
            }
        }

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        Ok(())
    }

    fn adjust_rate(&self, delta: i64) {
        let cur = self.shared.rate.load(Ordering::Relaxed) as i64;
        let new = (cur + delta).max(0) as u64;
        self.shared.rate.store(new, Ordering::Relaxed);
    }

    fn adjust_size(&self, delta: i64) {
        let cur = self.shared.msg_size.load(Ordering::Relaxed) as i64;
        let new = (cur + delta).max(50).min(65_000) as usize;
        self.shared.msg_size.store(new, Ordering::Relaxed);
    }

    // ── 렌더링 ────────────────────────────────────────────────────────────

    fn render(&self, f: &mut Frame) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),  // 설정 제어 패널
                Constraint::Length(5),  // syslog 통계 | 지연 시간
                Constraint::Length(4),  // CPU | 메모리
                Constraint::Length(5),  // I/O | 컨텍스트 스위치 | syslog 데몬
                Constraint::Min(3),     // 전송률 히스토리
                Constraint::Length(3),  // 단축키
            ])
            .split(area);

        self.render_config(f, chunks[0]);
        self.render_stats_row(f, chunks[1]);
        self.render_system_row(f, chunks[2]);
        self.render_extended_row(f, chunks[3]);
        self.render_history(f, chunks[4]);
        self.render_help(f, chunks[5]);
    }

    // ── 설정 제어 패널 ────────────────────────────────────────────────────

    fn render_config(&self, f: &mut Frame, area: Rect) {
        let rate = self.shared.rate.load(Ordering::Relaxed);
        let size = self.shared.msg_size.load(Ordering::Relaxed);
        let paused = self.shared.paused.load(Ordering::Relaxed);
        let threads = self.shared.num_threads;

        let (status_str, status_color) = if paused {
            ("⏸ 일시정지", Color::Yellow)
        } else {
            ("▶ 실행 중", Color::Green)
        };

        let rate_str = if rate == 0 { "최대".to_string() } else { format!("{}", rate) };

        let lines = vec![
            Line::from(vec![
                Span::raw("  전송률: "),
                Span::styled(
                    format!("{:>8} logs/sec", rate_str),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw("   ↑↓: ±100  ←→: ±1000  PgUp/Dn: ×10/÷10"),
            ]),
            Line::from(vec![
                Span::raw("  메시지: "),
                Span::styled(
                    format!("{:>8} bytes", size),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw("   [/]: ±64  {/}: ±512"),
            ]),
            Line::from(vec![
                Span::raw("  facility: "),
                Span::styled(&self.facility_name, Style::default().fg(Color::White)),
                Span::raw("  severity: "),
                Span::styled(&self.severity_name, Style::default().fg(Color::White)),
                Span::raw("  threads: "),
                Span::styled(format!("{}", threads), Style::default().fg(Color::White)),
                Span::raw("     상태: "),
                Span::styled(status_str, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
                Span::raw("  [Space]"),
            ]),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" ⚙ 설정 제어 (실시간 조정 가능) ")
            .title_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new(lines).block(block);
        f.render_widget(para, area);
    }

    // ── syslog 통계 | 전송 지연 ───────────────────────────────────────────

    fn render_stats_row(&self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        self.render_syslog_stats(f, cols[0]);
        self.render_latency(f, cols[1]);
    }

    fn render_syslog_stats(&self, f: &mut Frame, area: Rect) {
        let total = self.stats.total_sent.load(Ordering::Relaxed);
        let failed = self.stats.total_failed.load(Ordering::Relaxed);
        let bytes = self.stats.bytes_sent.load(Ordering::Relaxed);
        let rate = self.history.back().copied().unwrap_or(0.0) as u64;
        let success_rate = if total + failed > 0 {
            total as f64 / (total + failed) as f64 * 100.0
        } else { 100.0 };

        let lines = vec![
            Line::from(vec![
                Span::raw("  전송: "),
                Span::styled(format!("{:>12}", total), Style::default().fg(Color::Green)),
                Span::raw("  실패: "),
                Span::styled(format!("{:>8}", failed), Style::default().fg(Color::Red)),
            ]),
            Line::from(vec![
                Span::raw("  속도: "),
                Span::styled(format!("{:>8} logs/sec", rate), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::raw("  총량: "),
                Span::styled(fmt_bytes(bytes), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::raw("  성공: "),
                Span::styled(
                    format!("{:>6.2}%", success_rate),
                    Style::default().fg(if failed > 0 { Color::Red } else { Color::Green }),
                ),
            ]),
        ];

        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Syslog 전송 통계 "));
        f.render_widget(para, area);
    }

    fn render_latency(&self, f: &mut Frame, area: Rect) {
        let avg = self.stats.avg_latency_us.load(Ordering::Relaxed);
        let min_raw = self.stats.min_latency_us.load(Ordering::Relaxed);
        let max = self.stats.max_latency_us.load(Ordering::Relaxed);
        let min = if min_raw == u64::MAX { 0 } else { min_raw };

        let lat_color = |us: u64| {
            if us > 10_000 { Color::Red }
            else if us > 1_000 { Color::Yellow }
            else { Color::Green }
        };

        let lines = vec![
            Line::from(vec![
                Span::raw("  평균: "),
                Span::styled(fmt_latency(avg), Style::default().fg(lat_color(avg))),
            ]),
            Line::from(vec![
                Span::raw("  최소: "),
                Span::styled(fmt_latency(min), Style::default().fg(Color::Green)),
            ]),
            Line::from(vec![
                Span::raw("  최대: "),
                Span::styled(fmt_latency(max), Style::default().fg(lat_color(max))),
            ]),
            Line::from(Span::styled(
                "  (최근 1초 syslog() 호출 지연)",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" 전송 지연 "));
        f.render_widget(para, area);
    }

    // ── CPU | 메모리 ──────────────────────────────────────────────────────

    fn render_system_row(&self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        self.render_cpu(f, cols[0]);
        self.render_memory(f, cols[1]);
    }

    fn render_cpu(&self, f: &mut Frame, area: Rect) {
        let s = &self.sys_stats;
        let block = Block::default().borders(Borders::ALL).title(" CPU ");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);

        let sys_color = gauge_color(s.cpu_usage_pct);
        let proc_color = gauge_color(s.proc_cpu_pct);

        f.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(sys_color).bg(Color::Black))
                .percent(s.cpu_usage_pct.min(100.0) as u16)
                .label(format!("시스템 {:5.1}%", s.cpu_usage_pct)),
            rows[0],
        );
        f.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(proc_color).bg(Color::Black))
                .percent(s.proc_cpu_pct.min(100.0) as u16)
                .label(format!("프로세스{:5.1}%", s.proc_cpu_pct)),
            rows[1],
        );
    }

    fn render_memory(&self, f: &mut Frame, area: Rect) {
        let s = &self.sys_stats;
        let mem_pct = if s.mem_total_kb > 0 {
            s.mem_used_kb as f64 / s.mem_total_kb as f64 * 100.0
        } else { 0.0 };

        let block = Block::default().borders(Borders::ALL).title(" 메모리 ");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);

        f.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(gauge_color(mem_pct)).bg(Color::Black))
                .percent(mem_pct.min(100.0) as u16)
                .label(format!(
                    "{}/{} ({:.0}%)",
                    fmt_bytes(s.mem_used_kb * 1024),
                    fmt_bytes(s.mem_total_kb * 1024),
                    mem_pct,
                )),
            rows[0],
        );
        f.render_widget(
            Paragraph::new(format!(" RSS: {}", fmt_bytes(s.proc_vm_rss_kb * 1024)))
                .style(Style::default().fg(Color::White)),
            rows[1],
        );
    }

    // ── I/O | 컨텍스트 스위치 | syslog 데몬 ──────────────────────────────

    fn render_extended_row(&self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(34),
                Constraint::Percentage(33),
            ])
            .split(area);

        self.render_io_stats(f, cols[0]);
        self.render_ctxt_stats(f, cols[1]);
        self.render_syslogd(f, cols[2]);
    }

    fn render_io_stats(&self, f: &mut Frame, area: Rect) {
        let s = &self.sys_stats;
        // syslog() 호출수/sec와 전송 바이트/sec는 Stats에서 직접 계산
        // (/proc/self/io syscw는 Unix 소켓 write를 집계하지 않으므로 사용 불가)
        let call_rate = self.history.back().copied().unwrap_or(0.0) as u64;
        let lines = vec![
            Line::from(vec![
                Span::raw(" 호출: "),
                Span::styled(
                    format!("{}/s", call_rate),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(" ← syslog()", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::raw(" 송신: "),
                Span::styled(
                    format!("{}/s", fmt_bytes(self.bytes_rate)),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(" ← 페이로드", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::raw(" 로그파일: "),
                Span::styled(
                    format!("{}/s", fmt_bytes(s.syslogd_write_bytes_per_sec)),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(" ← rsyslogd", Style::default().fg(Color::DarkGray)),
            ]),
        ];
        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" syslog I/O "));
        f.render_widget(para, area);
    }

    fn render_ctxt_stats(&self, f: &mut Frame, area: Rect) {
        let s = &self.sys_stats;
        let lines = vec![
            Line::from(vec![
                Span::raw(" 자발적: "),
                Span::styled(
                    format!("{}/s", s.voluntary_ctxt_per_sec),
                    Style::default().fg(Color::Green),
                ),
            ]),
            Line::from(vec![
                Span::raw(" 비자발: "),
                Span::styled(
                    format!("{}/s", s.nonvoluntary_ctxt_per_sec),
                    Style::default().fg(Color::Red),
                ),
            ]),
            Line::from(Span::styled(
                " (컨텍스트 스위치/sec)",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" 컨텍스트 스위치 "));
        f.render_widget(para, area);
    }

    fn render_syslogd(&self, f: &mut Frame, area: Rect) {
        let s = &self.sys_stats;
        let (name, cpu_line, rss_line) = if s.syslogd_name.is_empty() {
            (
                "미감지",
                Line::from(Span::styled(" 실행 중이 아님", Style::default().fg(Color::DarkGray))),
                Line::from(""),
            )
        } else {
            (
                s.syslogd_name.as_str(),
                Line::from(vec![
                    Span::raw(" CPU: "),
                    Span::styled(
                        format!("{:.1}%", s.syslogd_cpu_pct),
                        Style::default().fg(gauge_color(s.syslogd_cpu_pct)),
                    ),
                ]),
                Line::from(vec![
                    Span::raw(" RSS: "),
                    Span::styled(
                        fmt_bytes(s.syslogd_rss_kb * 1024),
                        Style::default().fg(Color::White),
                    ),
                ]),
            )
        };

        let title = format!(" {} ", name);
        let lines = vec![cpu_line, rss_line, Line::from("")];
        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title));
        f.render_widget(para, area);
    }

    // ── 전송률 히스토리 (멀티행 바 차트) ─────────────────────────────────

    fn render_history(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 전송률 히스토리 (최근 120초, ▁▂▃▄▅▆▇█) ");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let height = inner.height as usize;
        let width = inner.width as usize;
        if height == 0 || width == 0 || self.history.is_empty() {
            return;
        }

        let max_val = self.history.iter().cloned().fold(0.0f64, f64::max).max(1.0);

        // 표시할 데이터 (최근 width 샘플) — VecDeque는 슬라이스 인덱싱 불가, skip으로 처리
        let skip = self.history.len().saturating_sub(width);
        let display: Vec<f64> = self.history.iter().copied().skip(skip).collect();

        // 바 차트 높이: 아래 1행은 레이블용으로 남김
        let chart_h = height.saturating_sub(1);

        let bar_chars = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

        // row 0 = top, chart_h-1 = bottom
        let mut lines: Vec<Line> = (0..chart_h)
            .map(|row| {
                let row_from_bottom = chart_h - 1 - row;
                let spans: Vec<Span> = display
                    .iter()
                    .map(|&v| {
                        let ratio = v / max_val;
                        // 이 열의 충진량 (1/8 단위)
                        let col_units = (ratio * (chart_h * 8) as f64).round() as usize;
                        let row_min_units = row_from_bottom * 8;

                        let ch = if col_units >= row_min_units + 8 {
                            '█'
                        } else if col_units > row_min_units {
                            let level = col_units - row_min_units;
                            bar_chars[level.min(8)]
                        } else {
                            ' '
                        };

                        let color = if ratio > 0.8 { Color::Red }
                            else if ratio > 0.5 { Color::Yellow }
                            else { Color::Green };
                        Span::styled(ch.to_string(), Style::default().fg(color))
                    })
                    .collect();

                // 최상단 행에 max 레이블 오버레이
                if row == 0 {
                    let label = format!("{:.0} logs/s", max_val);
                    let mut all_spans = spans;
                    // 레이블은 별도 Paragraph로 그리지 않고, max값만 오른쪽 정렬로 표기
                    all_spans.push(Span::styled(
                        format!(" ← max {}", label),
                        Style::default().fg(Color::DarkGray),
                    ));
                    Line::from(all_spans)
                } else {
                    Line::from(spans)
                }
            })
            .collect();

        // 하단 레이블 행
        let current = self.history.back().copied().unwrap_or(0.0) as u64;
        lines.push(Line::from(vec![
            Span::styled(
                format!(" 현재: {} logs/s", current),
                Style::default().fg(Color::Cyan),
            ),
        ]));

        f.render_widget(Paragraph::new(lines), inner);
    }

    // ── 단축키 안내 ───────────────────────────────────────────────────────

    fn render_help(&self, f: &mut Frame, area: Rect) {
        let key = |s: &str| Span::styled(s.to_string(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        let sep = || Span::raw("  ");

        let line = Line::from(vec![
            key(" ↑↓ "), Span::raw(":rate±100"),  sep(),
            key(" ←→ "), Span::raw(":rate±1000"), sep(),
            key(" PgUp/Dn "), Span::raw(":×10÷10"), sep(),
            key(" [/] "), Span::raw(":size±64"), sep(),
            key(" {/} "), Span::raw(":size±512"), sep(),
            key(" Spc "), Span::raw(":일시정지"), sep(),
            key(" q "), Span::raw(":종료"),
        ]);

        let para = Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL).title(" 단축키 "));
        f.render_widget(para, area);
    }
}

// ---------------------------------------------------------------------------
// 유틸
// ---------------------------------------------------------------------------

fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn fmt_latency(us: u64) -> String {
    if us >= 1_000_000 {
        format!("{:.2} s ", us as f64 / 1_000_000.0)
    } else if us >= 1_000 {
        format!("{:.2} ms", us as f64 / 1_000.0)
    } else {
        format!("{} μs", us)
    }
}

fn gauge_color(pct: f64) -> Color {
    if pct > 80.0 { Color::Red }
    else if pct > 50.0 { Color::Yellow }
    else { Color::Green }
}
