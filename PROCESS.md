# PROCESS.md

## 작업 이력

---

### 2026-03-16 — 작업 시작

**목표**: Linux syslog 부하 생성 CLI/TUI 프로그램 개발

**현황**:
- 프로젝트 디렉토리: `/home/yooseongc/syslog-generator`
- CLAUDE.md만 존재, 초기 상태

---

### 2026-03-16 — PLAN 수립 완료

**아키텍처**:
- Rust 1.93, musl libc 빌드
- `libc` crate로 syslog syscall (openlog/syslog/closelog) 직접 호출
- `ratatui` + `crossterm` 기반 TUI
- 설정: 전송 속도(N logs/sec), 로그 크기(K bytes), 로그 facility/severity
- 시스템 부하 측정: `/proc/stat`, `/proc/self/status` 파싱

**주요 모듈**:
1. `main.rs` — CLI 인자 파싱 + TUI 진입점
2. `syslog.rs` — libc syslog 래퍼
3. `generator.rs` — 로그 생성 루프 (rate control)
4. `monitor.rs` — 시스템 부하 측정
5. `tui.rs` — ratatui TUI 렌더링

**빌드 타겟**: `x86_64-unknown-linux-musl`

---

### 2026-03-16 — 개발 시작

- Cargo 프로젝트 초기화 및 의존성 설정
- syslog 래퍼, 생성기, 모니터, TUI 구현

---

### 2026-03-16 — 개발 종료 (v0.1.0)

**완료 항목**:
- `src/syslog_writer.rs`: libc openlog/syslog/closelog 래퍼, Facility/Severity 타입
- `src/generator.rs`: rate 제어 생성 루프, 원자적 통계(Stats), 멀티스레드 지원
- `src/monitor.rs`: /proc/stat CPU, /proc/meminfo 메모리, /proc/self/stat 프로세스 CPU 측정
- `src/tui.rs`: ratatui TUI — syslog 통계 패널, CPU/메모리 게이지, 전송률 히스토리 바 그래프
- `src/main.rs`: clap CLI, TUI/CLI 모드 분기, SIGINT 핸들러, 멀티스레드 생성기

**빌드 결과**:
- `target/x86_64-unknown-linux-musl/release/syslog-gen` — 1.2MB, static-pie, stripped
- 경고 0, 에러 0

**문서**:
- CLI.md: 사용법, 옵션, 시험 시나리오

---

### 2026-03-16 — v0.2.0 개선 개발 시작

**요청 사항**:
1. TUI에서 실시간 설정 제어 (rate/size 키보드 조정, pause/resume)
2. syslog 관련 추가 지표 추가
3. 메시지 랜덤화 (rsyslog "message repeated" 압축 방지)

---

### 2026-03-16 — 개발 종료 (v0.2.0)

**변경 사항**:
- `generator.rs`:
  - `SharedConfig` 추가 (AtomicU64 rate, AtomicUsize msg_size, AtomicBool paused)
  - `Stats`에 latency 필드 추가 (avg/min/max_latency_us, 최근 1초 윈도우)
  - LCG 기반 랜덤 패딩 (PaddingBuf) — 매 전송마다 일부 갱신
  - `seq=NNNNNNNNNN`을 메시지 앞쪽에 배치 → rsyslog 중복 압축 방지
  - `run_generator`가 SharedConfig를 실시간으로 읽어 rate/size 즉시 반영
- `monitor.rs`:
  - `/proc/self/io` — 프로세스 쓰기 바이트/sec, 쓰기 syscall/sec
  - `/proc/PID/status` — 컨텍스트 스위치(자발/비자발) per sec
  - `/proc/diskstats` — 물리 디스크 쓰기 bytes/sec
  - syslog 데몬 (rsyslogd/syslogd/syslog-ng) CPU%, RSS 자동 감지
  - `num_cpus` 불필요 필드 제거 (dead_code 억제 없이 완전 제거)
- `tui.rs`:
  - 설정 제어 패널 (현재 rate/size 표시, 키보드 조작 안내)
  - 키 바인딩: ↑↓ (±100), ←→ (±1000), PgUp/Dn (×10/÷10), [/] (±64 bytes), {/} (±512 bytes), Space (pause)
  - 전송 지연 패널 (avg/min/max)
  - 프로세스 I/O 패널 (쓰기 bytes/sec, syscall/sec)
  - 컨텍스트 스위치 패널
  - syslog 데몬 패널
  - 히스토리 차트: 단일행 → 멀티행 바 차트 (Unicode 블록 문자, 120초)
- `main.rs`: SharedConfig 통합, thread_id로 seq 공간 분리

**빌드 결과**: 경고 0, 에러 0

---

### 2026-03-16 — v0.2.1 버그 수정 (코드 리뷰 반영)

**수정 항목**:
- `Cargo.toml`: 사용하지 않는 `tokio` 의존성 제거
- `syslog_writer.rs`:
  - `CString::new().into_raw()` → `const IDENT: &[u8]` static 포인터 사용 (스레드마다 메모리 누수 해결)
  - `CString::new("%s")` → `const FMT_S: &[u8]` static 포인터 사용 (매 write() 할당 제거)
  - `Drop`에서 `closelog()` 제거 (프로세스 전역 상태 — 멀티스레드 경쟁 조건 방지)
- `main.rs`:
  - 시그널 핸들러: `Arc::into_raw(Arc::clone())` + `mem::forget` → `Arc::as_ptr` 사용 (signal handler에서 참조 카운트 조작 제거)
- `generator.rs`:
  - `running.load(Ordering::Relaxed)` → `Ordering::Acquire` (메모리 순서 보장)
  - min/max latency 갱신: load-check-store → `compare_exchange_weak` 루프 (멀티스레드 경쟁 조건 해결)
  - `make_interval`: `Duration::from_nanos(0)` 방지를 위해 `.max(1)` 추가
- `monitor.rs`:
  - `sysconf(_SC_CLK_TCK)` 오류(-1) 처리 → 기본값 100 fallback
  - 컨텍스트 스위치/sec 상한 `1_000_000` 적용 (elapsed가 매우 짧을 때 오버플로 방지)
- `tui.rs`:
  - `Vec<f64>` → `VecDeque<f64>` 전환 (`remove(0)` O(n) → `pop_front()` O(1))
  - `history.last()` → `history.back()` (VecDeque API)

**빌드 결과**: 경고 0, 에러 0

---

### 2026-03-16 — v1.0.0 2차 코드 리뷰 반영 및 정식 버전 출시

**추가 수정 항목**:
- `monitor.rs`:
  - `find_syslogd()`: `fname.to_str()?` → `match ... { None => continue }` (비UTF-8 파일명 시 함수 전체 조기 종료 버그 수정)
  - `read_proc_ticks()`: `content.find(')')` → `content.rfind(')')` (comm 필드에 ')'가 포함된 프로세스명 오파싱 방지; 주석과 코드 불일치 수정)
- `main.rs`:
  - `running.store(false, Ordering::Relaxed)` → `Ordering::Release` (CLI/TUI 정상 종료 시 generator Acquire 로드와 올바른 메모리 순서 보장)
  - 버전 `0.2.0` → `1.0.0`
- `syslog_writer.rs`:
  - `std::sync::Once` 도입: 멀티스레드가 동시에 `SyslogWriter::new()` 호출 시 `openlog()` 경쟁 조건 해결. 프로세스 전체에서 정확히 한 번만 호출됨
- `tui.rs`:
  - 전송률/바이트 델타 계산에 `saturating_sub` 적용 (방어적 처리)
- `Cargo.toml`: 버전 `0.1.0` → `1.0.0`

**빌드 결과**: 경고 0, 에러 0

