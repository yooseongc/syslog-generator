use std::ffi::CString;
use std::sync::Once;

// openlog에 전달할 ident — 프로세스 수명 동안 유효한 static NUL-종료 바이트 슬라이스
const IDENT: &[u8] = b"syslog-gen\0";

// write()에서 매번 CString 할당을 피하기 위한 고정 포맷 문자열
const FMT_S: &[u8] = b"%s\0";

// openlog()는 프로세스 전역 상태를 설정하며 thread-safe하지 않음.
// Once를 사용하여 멀티스레드 환경에서 정확히 한 번만 호출함.
static OPENLOG_ONCE: Once = Once::new();

/// syslog facility 값 (RFC 3164)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Facility {
    Kern = 0,
    User = 1,
    Mail = 2,
    Daemon = 3,
    Auth = 4,
    Syslog = 5,
    Lpr = 6,
    News = 7,
    Uucp = 8,
    Cron = 9,
    AuthPriv = 10,
    Ftp = 11,
    Local0 = 16,
    Local1 = 17,
    Local2 = 18,
    Local3 = 19,
    Local4 = 20,
    Local5 = 21,
    Local6 = 22,
    Local7 = 23,
}

pub const ALL_FACILITIES: &[Facility] = &[
    Facility::Kern, Facility::User, Facility::Mail, Facility::Daemon,
    Facility::Auth, Facility::Syslog, Facility::Lpr, Facility::News,
    Facility::Uucp, Facility::Cron, Facility::AuthPriv, Facility::Ftp,
    Facility::Local0, Facility::Local1, Facility::Local2, Facility::Local3,
    Facility::Local4, Facility::Local5, Facility::Local6, Facility::Local7,
];

impl Facility {
    pub fn as_u8(self) -> u8 { self as u8 }
    pub fn from_u8(v: u8) -> Option<Self> {
        ALL_FACILITIES.iter().copied().find(|f| *f as u8 == v)
    }
}

/// syslog severity (priority) 값
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

pub const ALL_SEVERITIES: &[Severity] = &[
    Severity::Emergency, Severity::Alert, Severity::Critical, Severity::Error,
    Severity::Warning, Severity::Notice, Severity::Info, Severity::Debug,
];

impl Severity {
    pub fn as_u8(self) -> u8 { self as u8 }
    pub fn from_u8(v: u8) -> Option<Self> {
        ALL_SEVERITIES.iter().copied().find(|s| *s as u8 == v)
    }
}

pub struct SyslogWriter {
    facility: Facility,
    severity: Severity,
}

impl SyslogWriter {
    pub fn new(facility: Facility, severity: Severity) -> Self {
        OPENLOG_ONCE.call_once(|| {
            unsafe {
                // LOG_NDELAY(0x08) | LOG_PID(0x01)
                // IDENT는 static → openlog() 이후에도 포인터가 항상 유효
                libc::openlog(IDENT.as_ptr() as *const libc::c_char, 0x08 | 0x01, (facility as i32) << 3);
            }
        });
        Self { facility, severity }
    }

    /// 메시지를 syslog로 전송. 성공 시 true 반환.
    pub fn write(&self, message: &str) -> bool {
        let priority = ((self.facility as i32) << 3) | (self.severity as i32);
        match CString::new(message) {
            Ok(msg) => {
                unsafe {
                    // format "%s"로 고정하여 format string injection 방지
                    libc::syslog(priority, FMT_S.as_ptr() as *const libc::c_char, msg.as_ptr());
                }
                true
            }
            Err(_) => false,
        }
    }
}
// Drop을 구현하지 않음: openlog/closelog는 프로세스 전역 상태이며,
// 멀티스레드 환경에서 closelog()를 개별 스레드에서 호출하면 경쟁 조건 발생.

impl std::str::FromStr for Facility {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "kern" => Ok(Facility::Kern),
            "user" => Ok(Facility::User),
            "mail" => Ok(Facility::Mail),
            "daemon" => Ok(Facility::Daemon),
            "auth" => Ok(Facility::Auth),
            "syslog" => Ok(Facility::Syslog),
            "lpr" => Ok(Facility::Lpr),
            "news" => Ok(Facility::News),
            "uucp" => Ok(Facility::Uucp),
            "cron" => Ok(Facility::Cron),
            "authpriv" => Ok(Facility::AuthPriv),
            "ftp" => Ok(Facility::Ftp),
            "local0" => Ok(Facility::Local0),
            "local1" => Ok(Facility::Local1),
            "local2" => Ok(Facility::Local2),
            "local3" => Ok(Facility::Local3),
            "local4" => Ok(Facility::Local4),
            "local5" => Ok(Facility::Local5),
            "local6" => Ok(Facility::Local6),
            "local7" => Ok(Facility::Local7),
            _ => Err(format!("Unknown facility: {}", s)),
        }
    }
}

impl std::fmt::Display for Facility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Facility::Kern => "kern",
            Facility::User => "user",
            Facility::Mail => "mail",
            Facility::Daemon => "daemon",
            Facility::Auth => "auth",
            Facility::Syslog => "syslog",
            Facility::Lpr => "lpr",
            Facility::News => "news",
            Facility::Uucp => "uucp",
            Facility::Cron => "cron",
            Facility::AuthPriv => "authpriv",
            Facility::Ftp => "ftp",
            Facility::Local0 => "local0",
            Facility::Local1 => "local1",
            Facility::Local2 => "local2",
            Facility::Local3 => "local3",
            Facility::Local4 => "local4",
            Facility::Local5 => "local5",
            Facility::Local6 => "local6",
            Facility::Local7 => "local7",
        };
        write!(f, "{}", s)
    }
}

impl std::str::FromStr for Severity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "emerg" | "emergency" => Ok(Severity::Emergency),
            "alert" => Ok(Severity::Alert),
            "crit" | "critical" => Ok(Severity::Critical),
            "err" | "error" => Ok(Severity::Error),
            "warn" | "warning" => Ok(Severity::Warning),
            "notice" => Ok(Severity::Notice),
            "info" => Ok(Severity::Info),
            "debug" => Ok(Severity::Debug),
            _ => Err(format!("Unknown severity: {}", s)),
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Severity::Emergency => "emerg",
            Severity::Alert => "alert",
            Severity::Critical => "crit",
            Severity::Error => "err",
            Severity::Warning => "warn",
            Severity::Notice => "notice",
            Severity::Info => "info",
            Severity::Debug => "debug",
        };
        write!(f, "{}", s)
    }
}
