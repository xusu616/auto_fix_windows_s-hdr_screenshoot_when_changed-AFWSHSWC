use serde_json::{json, Value};
use std::io::{self, Error};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tungstenite::{client, Message, WebSocket};
use windows::core::PWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONULL};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const IO_TIMEOUT: Duration = Duration::from_millis(800);
const CONSENT_TIMEOUT: Duration = Duration::from_secs(20);
const RECONNECT_DELAY: Duration = Duration::from_secs(15);
const ACTIVE_PLAYER: &str = r#"(() => {
    if (location.origin !== 'https://www.bilibili.com' ||
        document.hidden || !document.hasFocus()) return 'inactive';
    for (const video of document.querySelectorAll('video')) {
        const { top, left, bottom, right, width, height } = video.getBoundingClientRect();
        if (video.readyState < 2 || video.ended || width === 0 || height === 0 ||
            bottom <= 0 || right <= 0 || top >= innerHeight || left >= innerWidth) continue;
        const player = video.closest('.bpx-player-container, .bilibili-player, #bilibili-player');
        const controls = (player || document).querySelectorAll('[class*="quality-result"]');
        for (const control of controls) {
            if (/\bHDR\s*10\b|HDR\s*真彩/i.test(control.textContent)) return 'hdr';
        }
    }
    return 'active';
})()"#;

pub struct Chrome {
    connection: Option<Cdp>,
    retry_after: Option<Instant>,
    last_error: Option<String>,
    foreground: bool,
    hdr: bool,
}

impl Chrome {
    pub fn new() -> Self {
        Self {
            connection: None,
            retry_after: None,
            last_error: None,
            foreground: false,
            hdr: false,
        }
    }

    pub fn status(&self) -> &str {
        if !self.foreground {
            "Chrome is outside the active display"
        } else if let Some(error) = &self.last_error {
            error
        } else if self.hdr {
            "Bilibili HDR10"
        } else {
            "No focused Bilibili HDR10 player"
        }
    }

    pub fn hdr_is_visible(&mut self, monitor: HMONITOR) -> bool {
        self.foreground = foreground_is_chrome(monitor);
        if !self.foreground || self.retry_after.is_some_and(|time| time > Instant::now()) {
            return false;
        }
        let result = match &mut self.connection {
            Some(connection) => connection.hdr_is_visible(),
            empty => {
                Cdp::connect().and_then(|connection| empty.insert(connection).hdr_is_visible())
            }
        };
        match result {
            Ok(hdr) => {
                self.last_error = None;
                self.hdr = hdr;
                hdr
            }
            Err(error) => {
                // Chrome may close the local debugging endpoint or await user consent.
                self.connection = None;
                self.retry_after = Some(Instant::now() + RECONNECT_DELAY);
                self.last_error = Some(error.to_string());
                false
            }
        }
    }
}

fn foreground_is_chrome(monitor: HMONITOR) -> bool {
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() || MonitorFromWindow(window, MONITOR_DEFAULTTONULL) != monitor {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut path = [0u16; 1024];
        let mut length = path.len() as u32;
        let image = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        image.is_ok()
            && String::from_utf16_lossy(&path[..length as usize])
                .rsplit('\\')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("chrome.exe"))
    }
}

struct Cdp {
    socket: WebSocket<TcpStream>,
    request_id: u64,
    session: Option<String>,
}

impl Cdp {
    fn connect() -> io::Result<Self> {
        let dir = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| Error::other("LocalAppData is unavailable"))?;
        let endpoint = std::fs::read_to_string(
            PathBuf::from(dir).join(r"Google\Chrome\User Data\DevToolsActivePort"),
        )?;
        let mut lines = endpoint.lines();
        let port = lines.next().and_then(|line| line.parse::<u16>().ok());
        let path = lines.next().filter(|line| {
            line.strip_prefix("/devtools/browser/").is_some_and(|id| {
                !id.is_empty() && id.bytes().all(|ch| ch.is_ascii_hexdigit() || ch == b'-')
            })
        });
        let (Some(port), Some(path)) = (port, path) else {
            return Err(Error::other("Chrome DevTools endpoint is invalid"));
        };
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
        stream.set_read_timeout(Some(CONSENT_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let url = format!("ws://{address}{path}");
        let (socket, _) = client(&url, stream)
            .map_err(|error| Error::other(format!("Chrome WebSocket handshake: {error}")))?;
        socket.get_ref().set_read_timeout(Some(IO_TIMEOUT))?;
        Ok(Self {
            socket,
            request_id: 0,
            session: None,
        })
    }

    fn hdr_is_visible(&mut self) -> io::Result<bool> {
        if let Some(session) = self.session.take() {
            if let Some(hdr) = self.playback(&session)? {
                self.session = Some(session);
                return Ok(hdr);
            }
            self.detach(&session)?;
        }
        let targets = self.call("Target.getTargets", json!({}), None)?;
        let Some(pages) = targets["targetInfos"].as_array() else {
            return Ok(false);
        };
        for page in pages {
            let url = page["url"].as_str().unwrap_or_default();
            if page["type"] != "page" || !url.starts_with("https://www.bilibili.com/") {
                continue;
            }
            let attachment = self.call(
                "Target.attachToTarget",
                json!({"targetId": page["targetId"], "flatten": true}),
                None,
            )?;
            if let Some(session) = attachment["sessionId"].as_str() {
                if let Some(hdr) = self.playback(session)? {
                    self.session = Some(session.into());
                    return Ok(hdr);
                }
                self.detach(session)?;
            }
        }
        Ok(false)
    }

    fn playback(&mut self, session: &str) -> io::Result<Option<bool>> {
        let response = self.call(
            "Runtime.evaluate",
            json!({"expression": ACTIVE_PLAYER, "returnByValue": true}),
            Some(session),
        )?;
        Ok(match response["result"]["value"].as_str() {
            Some("hdr") => Some(true),
            Some("active") => Some(false),
            _ => None,
        })
    }

    fn detach(&mut self, session: &str) -> io::Result<()> {
        self.call(
            "Target.detachFromTarget",
            json!({"sessionId": session}),
            None,
        )?;
        Ok(())
    }

    fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> io::Result<Value> {
        self.request_id += 1;
        let mut message = json!({"id": self.request_id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::text(message.to_string()))
            .map_err(|error| Error::other(format!("{method}: {error}")))?;
        loop {
            let reply = self
                .socket
                .read()
                .map_err(|error| Error::other(format!("{method}: {error}")))?;
            if let Message::Text(content) = reply {
                let decoded: Value = serde_json::from_str(&content)?;
                if decoded["id"].as_u64() == Some(self.request_id) {
                    return Ok(decoded["result"].clone());
                }
            }
        }
    }
}
