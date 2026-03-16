# syslog-gen

Linux syslog 부하 생성기. 전송 속도(N logs/sec)와 메시지 크기(K bytes)를 조절하며 시스템 부하를 재현하고, `/proc` 기반 실시간 모니터링을 제공하는 CLI/TUI 도구입니다.

## 다운로드

[Releases](https://github.com/yooseongc/syslog-generator/releases/latest) 페이지에서 사전 빌드된 바이너리를 받을 수 있습니다.

```bash
# x86_64 Linux (musl 정적 링크, 의존성 없음)
curl -L https://github.com/yooseongc/syslog-generator/releases/latest/download/syslog-gen-x86_64-linux-musl -o syslog-gen
chmod +x syslog-gen
```

## 빌드

**사전 요구사항**

```bash
sudo apt-get install musl-tools
rustup target add x86_64-unknown-linux-musl
```

```bash
# musl 정적 빌드 (배포용)
cargo build --release --target x86_64-unknown-linux-musl

# 바이너리 위치
./target/x86_64-unknown-linux-musl/release/syslog-gen
```

## 사용법

### TUI 모드 (기본)

```bash
./syslog-gen
```

실행 중에 키보드로 전송 속도와 메시지 크기를 실시간 조정할 수 있습니다.

### CLI 모드 (--timeout 지정)

```bash
# 10초간 기본 설정으로 실행
./syslog-gen --timeout 10

# 1000 logs/sec, 512 bytes 메시지, 10초
./syslog-gen -r 1000 -s 512 -t 10

# 최대 속도, 4스레드, 30초
./syslog-gen -r 0 -n 4 -t 30
```

종료 후 최종 통계(전송 수, 실패 수, 총 전송량, 지연)를 출력합니다.

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

**facility**: `kern` `user` `mail` `daemon` `auth` `syslog` `local0`~`local7`

**severity**: `emerg` `alert` `crit` `err` `warn` `notice` `info` `debug`

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
| `Space` | 일시정지 / 재개 |
| `q` / `Ctrl+C` | 종료 |

## TUI 모니터링 지표

| 패널 | 지표 |
|------|------|
| 설정 제어 | 현재 rate, size, facility, severity, threads, 실행 상태 |
| Syslog 전송 통계 | 전송/실패 카운트, 현재 속도(logs/sec), 총 전송량, 성공률 |
| 전송 지연 | `syslog()` 호출 지연 avg/min/max (최근 1초 윈도우) |
| CPU | 시스템 전체 CPU%, 현재 프로세스 CPU% |
| 메모리 | 시스템 메모리 사용률, 프로세스 RSS |
| syslog I/O | syslog 호출수/sec, 페이로드 bytes/sec, rsyslogd 로그파일 쓰기/sec |
| 컨텍스트 스위치 | 자발적/비자발적 컨텍스트 스위치/sec |
| syslog 데몬 | rsyslogd/syslogd/syslog-ng 자동 감지 — CPU%, RSS |
| 전송률 히스토리 | 최근 120초 바 차트 (▁▂▃▄▅▆▇█) |

## 시험 시나리오

WSL2 환경에서는 먼저 rsyslog를 실행합니다.

```bash
sudo service rsyslog start
tail -f /var/log/syslog | grep syslog-gen
```

**기본 부하 단계별 측정**

```bash
./syslog-gen -r 100   -s 200  -t 30   # 낮은 속도, 작은 메시지
./syslog-gen -r 1000  -s 512  -t 30   # 중간 속도
./syslog-gen -r 5000  -s 1024 -t 30   # 높은 속도, 큰 메시지
./syslog-gen -r 0     -s 200  -t 10   # 최대 속도
```

**긴 메시지 병목 테스트** (커널/rsyslogd 처리 한계 측정)

```bash
./syslog-gen -r 1000 -s 4096  -t 30   # 4 KB 메시지
./syslog-gen -r 500  -s 8192  -t 30   # 8 KB 메시지
```

전송 지연(avg/max)이 급등하거나 실패 카운트가 증가하면 소켓 버퍼 포화 신호입니다.

**멀티스레드**

```bash
./syslog-gen -r 20000 -n 4 -t 30   # 4스레드로 총 20000 logs/sec
```

**TUI에서 실시간 관찰**

```bash
./syslog-gen -r 100 -s 200   # TUI 시작 후 ↑/→ 로 점진적으로 rate 증가
```

- rate를 올리며 rsyslogd CPU% 변화 관찰
- 메시지 크기를 늘리며 로그파일 쓰기 bytes/sec 변화 확인
- 전송 지연 max로 소켓 포화 여부 판단

## 구현 참고 사항

- **syslog 전달 방식**: `syslog(3)` libc 함수 → `/dev/log` Unix 도메인 소켓 → rsyslogd
- **syscall 계측**: `/proc/self/io`의 `syscw`는 Unix 소켓 write를 집계하지 않음. 내부 `total_sent` 카운터를 직접 사용
- **메시지 고유성**: `seq=NNNNNNNNNN`을 메시지 앞쪽에 배치 + LCG 랜덤 패딩으로 rsyslog "message repeated" 압축 방지
- **빌드 타겟**: `x86_64-unknown-linux-musl` — 정적 링크, 외부 의존성 없음

## 라이선스

MIT
