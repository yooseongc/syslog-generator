# NOTES.md

## 주의 사항

- `syslog(3)` 은 libc 함수이며, 내부적으로 `/dev/log` 소켓을 통해 syslogd에 메시지를 전달한다.
  - WSL2 환경에서는 `rsyslog` 또는 `syslog-ng`가 실행 중이어야 한다.
  - WSL2에서 테스트 시 `sudo service rsyslog start` 필요할 수 있음.
- musl libc 빌드 시 `x86_64-unknown-linux-musl` 타겟 사용.
  - `rustup target add x86_64-unknown-linux-musl` 필요.
  - `musl-tools` 패키지 설치 필요 (`apt install musl-tools`).
- `libc` crate의 `syslog` 함수는 variadic이라 Rust에서 직접 호출 시 주의.
  - format string을 `%s` 고정으로 하고 메시지를 CString으로 전달하는 방식 사용.
- 로그 크기(K bytes) 구현 시 메시지 패딩으로 목표 크기 맞춤.
  - syslog 메시지에는 헤더가 붙으므로 실제 전송 크기는 더 클 수 있음.
- TUI와 syslog 생성기는 별도 스레드로 동작 (Arc<Mutex> 또는 채널로 통신).

## /proc/self/io syscw는 syslog write를 집계하지 않음 (중요)

- `/proc/self/io`의 `syscw`는 VFS(파일시스템) 경로의 write syscall만 집계한다.
- `syslog()`는 내부적으로 `/dev/log` **Unix 도메인 소켓**에 `write()`/`sendmsg()`를 호출하는데,
  소켓 write는 `sock_sendmsg()` 경로를 사용하며 `task_io_accounting`을 증가시키지 않는다.
- 따라서 20000 syslog/sec를 보내도 `syscw`는 4 등 매우 낮게 표시됨.
- **올바른 syslog syscall 측정**: 우리 코드의 `total_sent` 카운터를 직접 사용해야 한다.
- **rsyslogd가 로그파일에 쓰는 양**: `/proc/rsyslogd_pid/io`의 `write_bytes`는 정상 집계됨
  (rsyslogd는 소켓에서 받아 실제 파일에 쓰므로 VFS 경로를 사용).

## 긴 메시지 병목 테스트 계획

- 목적: 엄청 긴 syslog 메시지(예: 4KB, 8KB, 64KB)가 커널 및 시스템 전체에 미치는 영향
- 관찰 지표:
  - syslog() 호출 지연 증가 (avg/max latency)
  - syslog() 소켓 버퍼 포화 시점 (max latency 급등, 실패 카운트 증가)
  - rsyslogd CPU 사용률 증가 (메시지 파싱/처리 부하)
  - rsyslogd 파일 쓰기 속도
  - 컨텍스트 스위치 증가
- Linux syslog 메시지 크기 제한: `/dev/log` Unix socket의 최대 datagram 크기는
  `net.unix.max_dgram_qlen`, `SO_SNDBUF` 등에 따라 제한될 수 있음.
  실제로는 rsyslog가 대형 메시지를 잘라낼 수 있음 (기본 8KB 제한 등).

## rsyslog UDP 전송 테스트 준비 사항

- rsyslog에서 UDP 전송 활성화 예시 (`/etc/rsyslog.conf`):
  ```
  *.* @127.0.0.1:514        # UDP 전송 (원격 syslog 서버로)
  # 또는 로컬에서 수신 활성화:
  module(load="imudp")
  input(type="imudp" port="514")
  ```
- UDP 테스트 시 추가로 관찰할 지표:
  - `/proc/net/udp` — UDP 수신 버퍼 드롭 (`drops` 컬럼)
  - `netstat -su` — UDP 수신 오류/버퍼 오버플로우
  - rsyslogd → 원격 서버 전송 시: 네트워크 대역폭, 패킷 드롭

## 협의 사항

- 2026-03-16: 초기 설계 확정
  - CLI 모드(TUI 없이 바로 실행)와 TUI 모드 모두 지원
  - 측정 항목: CPU 사용률, 메모리 사용량, syslog 전송 통계(성공/실패/초당 전송률)
- 2026-03-16: syslog 부하 측정 목적 구체화
  - 엄청 긴 메시지가 커널/시스템에 미치는 병목 측정
  - 향후 rsyslog UDP 전송 켜고 추가 테스트 예정
