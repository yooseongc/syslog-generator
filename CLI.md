# CLI.md — syslog-gen 사용법 및 시험 방법

## 빌드

```bash
# 개발 빌드
cargo build

# musl libc 정적 빌드 (배포용)
cargo build --release --target x86_64-unknown-linux-musl

# 바이너리 위치
# ./target/x86_64-unknown-linux-musl/release/syslog-gen
```

### 사전 요구사항

```bash
sudo apt-get install musl-tools
rustup target add x86_64-unknown-linux-musl
```

## 실행

### TUI 모드 (기본)

```bash
./syslog-gen
```

### CLI 모드 (timeout 지정)

```bash
./syslog-gen --timeout 10          # 10초 실행 후 종료
./syslog-gen -r 1000 -s 512 -t 10 # 1000 logs/sec, 512 bytes, 10초
```

## 옵션

| 옵션 | 설명 | 기본값 |
|------|------|--------|
| `-r, --rate <N>` | 초당 전송 수 (0 = 최대 속도) | `100` |
| `-s, --size <bytes>` | 메시지 목표 크기 (bytes) | `200` |
| `-f, --facility <name>` | syslog facility | `user` |
| `-l, --level <name>` | syslog severity | `info` |
| `-p, --prefix <str>` | 메시지 접두사 | `load-test` |
| `-t, --timeout <sec>` | CLI 모드 실행 시간 (0 = TUI 모드) | `0` |
| `-n, --threads <N>` | 생성기 스레드 수 | `1` |

### facility
`kern`, `user`, `mail`, `daemon`, `auth`, `syslog`, `lpr`, `news`, `uucp`, `cron`, `authpriv`, `ftp`, `local0`~`local7`

### severity
`emerg`, `alert`, `crit`, `err`, `warn`, `notice`, `info`, `debug`

## TUI 단축키

| 키 | 동작 |
|----|------|
| `↑` / `k` | rate +100 logs/sec |
| `↓` / `j` | rate -100 logs/sec |
| `→` / `l` | rate +1000 logs/sec |
| `←` / `h` | rate -1000 logs/sec |
| `PgUp` | rate ×10 |
| `PgDn` | rate ÷10 |
| `]` | 메시지 크기 +64 bytes |
| `[` | 메시지 크기 -64 bytes |
| `}` | 메시지 크기 +512 bytes |
| `{` | 메시지 크기 -512 bytes |
| `f` | facility 선택 팝업 열기 |
| `s` | severity 선택 팝업 열기 |
| `↑` / `↓` (팝업 내) | 항목 이동 |
| `Enter` (팝업 내) | 선택 적용 |
| `Esc` / `q` (팝업 내) | 팝업 닫기 (취소) |
| `Space` | 일시정지 / 재개 |
| `q` / `Ctrl+C` | 종료 |

## TUI 표시 지표

| 패널 | 지표 |
|------|------|
| 설정 제어 | 현재 rate, size, facility, severity, 실행 상태 |
| Syslog 전송 통계 | 전송/실패 카운트, 현재 속도, 총 전송량, 성공률 |
| 전송 지연 | syslog() 호출 지연: 평균/최소/최대 (최근 1초) |
| CPU | 시스템 전체 CPU%, 프로세스 CPU% |
| 메모리 | 시스템 메모리 사용률, 프로세스 RSS |
| 프로세스 I/O | /proc/self/io — 쓰기 바이트/sec, 쓰기 syscall/sec |
| 컨텍스트 스위치 | 자발적/비자발적 컨텍스트 스위치/sec |
| syslog 데몬 | rsyslogd/syslogd 자동 감지 — CPU%, RSS |
| 시스템 디스크 쓰기 | /proc/diskstats — 물리 디스크 쓰기 bytes/sec |
| 전송률 히스토리 | 최근 120초 bar chart (▁▂▃▄▅▆▇█) |

## 메시지 랜덤화

- 각 메시지는 `seq=0000000001` 을 앞쪽에 배치하여 rsyslog의 "message repeated" 압축 방지
- 패딩 내용은 LCG 기반 랜덤 소문자로 매 전송마다 일부 갱신
- 메시지가 rsyslog에서 잘리더라도 앞쪽의 seq로 고유성 보장

## 시험 방법

### 1. syslog 데몬 확인 (WSL2)

```bash
sudo service rsyslog start
tail -f /var/log/syslog | grep syslog-gen
```

### 2. 부하 테스트 시나리오

```bash
# 낮은 속도, 작은 메시지
./syslog-gen -r 100 -s 100 -t 30

# 중간 속도, 중간 메시지
./syslog-gen -r 1000 -s 512 -t 30

# 높은 속도, 큰 메시지
./syslog-gen -r 5000 -s 1024 -t 30

# 최대 속도
./syslog-gen -r 0 -s 200 -t 10

# 멀티스레드 (4스레드로 총 10000 logs/sec)
./syslog-gen -r 10000 -n 4 -t 30
```

### 3. TUI에서 실시간 조정

```bash
./syslog-gen -r 100 -s 200  # TUI 모드로 시작 후 키보드로 조정
```

- rate 0에서 시작 → `↑` 또는 `→` 로 점진적으로 올리면서 syslog 데몬 CPU 변화 관찰
- 메시지 크기를 늘리면 디스크 쓰기 bytes/sec 변화 확인
- 전송 지연(평균/최대)으로 syslog 소켓 포화 여부 판단
