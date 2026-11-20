use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::capture::{Measurement, Probe};
use crate::settings;
use crate::signal::{Evidence, Signal};

const APP_NAME: PCWSTR = w!("AFWSHSWC");
const WINDOW_CLASS: PCWSTR = w!("AFWSHSWC.Tray");
const APP_ICON: PCWSTR = PCWSTR(1 as *const u16);
const WM_TRAY: u32 = WM_APP + 1;
const SHOW: usize = 101;
const AUTO: usize = 102;
const ENABLE: usize = 103;
const DISABLE: usize = 104;
const QUIT: usize = 105;

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
enum Mode {
    Automatic,
    Manual(bool),
}

struct Ui {
    status_text: HWND,
    commands: Sender<Mode>,
    status: Receiver<String>,
    last_status: String,
    icon: NOTIFYICONDATAW,
    taskbar_created: u32,
}

struct MutexHandle(HANDLE);

impl Drop for MutexHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn show_error(message: &str) {
    let wide = encode(message);
    unsafe {
        MessageBoxW(None, PCWSTR(wide.as_ptr()), APP_NAME, MB_OK | MB_ICONERROR);
    }
}

pub fn run(chrome: bool) -> Result<()> {
    let mutex = MutexHandle(unsafe { CreateMutexW(None, false, w!("Local\\AFWSHSWC.Tray"))? });
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if let Ok(window) = unsafe { FindWindowW(WINDOW_CLASS, None) } {
            let _ = unsafe { PostMessageW(window, WM_COMMAND, WPARAM(SHOW), LPARAM(0)) };
        }
        return Ok(());
    }

    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let icon = unsafe { LoadIconW(instance, APP_ICON)? };
    let small_icon = unsafe {
        HICON(
            LoadImageW(
                instance,
                APP_ICON,
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_SHARED,
            )?
            .0,
        )
    };
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: icon,
        hIconSm: small_icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_win32());
    }
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WINDOW_CLASS,
            APP_NAME,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            450,
            242,
            None,
            None,
            instance,
            None,
        )?
    };
    let status_text = control(
        window,
        w!("STATIC"),
        w!("正在读取显示器和截图工具设置…"),
        20,
        18,
        399,
        120,
        0,
    )?;
    control(window, w!("BUTTON"), w!("自动判断"), 20, 146, 116, 30, AUTO)?;
    control(
        window,
        w!("BUTTON"),
        w!("手动开启"),
        154,
        146,
        116,
        30,
        ENABLE,
    )?;
    control(
        window,
        w!("BUTTON"),
        w!("手动关闭"),
        288,
        146,
        116,
        30,
        DISABLE,
    )?;

    let mut notification = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: small_icon,
        ..Default::default()
    };
    let tip = encode("AFWSHSWC · 截图工具 HDR 矫正");
    notification.szTip[..tip.len()].copy_from_slice(&tip);
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &notification).as_bool() } {
        return Err(windows::core::Error::new(E_FAIL, "无法创建系统托盘图标"));
    }

    let (commands, command_rx) = mpsc::channel();
    let (status_tx, status) = mpsc::channel();
    UI.with(|ui| {
        ui.replace(Some(Ui {
            status_text,
            commands,
            status,
            last_status: String::new(),
            icon: notification,
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        }));
    });
    let worker = std::thread::spawn(move || monitor(command_rx, status_tx, chrome));
    let result = message_loop(window);
    let _ = unsafe { DestroyWindow(window) };
    UI.with(|ui| {
        ui.replace(None);
    });
    let _ = worker.join();
    drop(mutex);
    result
}

fn control(
    window: HWND,
    class: PCWSTR,
    title: PCWSTR,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: usize,
) -> Result<HWND> {
    let style = if id == 0 {
        WS_CHILD | WS_VISIBLE
    } else {
        WS_CHILD | WS_VISIBLE | WS_TABSTOP
    };
    let widget = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            title,
            style,
            x,
            y,
            width,
            height,
            window,
            HMENU(id as *mut _),
            None,
            None,
        )?
    };
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    unsafe {
        SendMessageW(widget, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }
    Ok(widget)
}

fn message_loop(window: HWND) -> Result<()> {
    if unsafe { SetTimer(window, 1, 200, None) } == 0 {
        return Err(windows::core::Error::from_win32());
    }
    let mut msg = MSG::default();
    loop {
        match unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 {
            -1 => return Err(windows::core::Error::from_win32()),
            0 => return Ok(()),
            _ => unsafe {
                if !IsDialogMessageW(window, &msg).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            },
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            let _ = ShowWindow(window, SW_HIDE);
        }
        WM_DESTROY => {
            UI.with(|ui| {
                if let Some(ui) = ui.borrow().as_ref() {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &ui.icon);
                }
            });
            PostQuitMessage(0);
        }
        WM_TIMER => update_status(),
        WM_COMMAND => command(window, wparam.0 & 0xffff),
        WM_TRAY => match lparam.0 as u32 {
            WM_LBUTTONUP => command(window, SHOW),
            WM_RBUTTONUP => command(window, tray_menu(window)),
            _ => {}
        },
        _ => {
            UI.with(|ui| {
                if let Some(ui) = ui.borrow().as_ref() {
                    if message == ui.taskbar_created {
                        let _ = Shell_NotifyIconW(NIM_ADD, &ui.icon);
                    }
                }
            });
            return DefWindowProcW(window, message, wparam, lparam);
        }
    }
    LRESULT(0)
}

fn command(window: HWND, id: usize) {
    let mode = match id {
        SHOW => {
            unsafe {
                let show = if IsIconic(window).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                };
                let _ = ShowWindow(window, show);
                let _ = SetForegroundWindow(window);
            }
            return;
        }
        QUIT => {
            unsafe {
                let _ = DestroyWindow(window);
            }
            return;
        }
        AUTO => Mode::Automatic,
        ENABLE => Mode::Manual(true),
        DISABLE => Mode::Manual(false),
        _ => return,
    };
    UI.with(|ui| {
        if let Some(ui) = ui.borrow().as_ref() {
            let _ = ui.commands.send(mode);
        }
    });
}

unsafe fn tray_menu(window: HWND) -> usize {
    let Ok(menu) = CreatePopupMenu() else {
        return 0;
    };
    let items = [
        (SHOW, w!("打开主界面")),
        (AUTO, w!("自动判断")),
        (ENABLE, w!("手动开启")),
        (DISABLE, w!("手动关闭")),
        (QUIT, w!("退出")),
    ];
    let mut point = POINT::default();
    let result = if items
        .into_iter()
        .all(|(id, label)| AppendMenuW(menu, MF_STRING, id, label).is_ok())
        && GetCursorPos(&mut point).is_ok()
    {
        let _ = SetForegroundWindow(window);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            window,
            None,
        );
        let _ = PostMessageW(window, WM_NULL, WPARAM(0), LPARAM(0));
        selected.0 as usize
    } else {
        0
    };
    let _ = DestroyMenu(menu);
    result
}

fn update_status() {
    let update = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let ui = ui.as_mut()?;
        let status = ui.status.try_iter().last()?;
        if status == ui.last_status {
            return None;
        }
        ui.last_status = status;
        Some((ui.status_text, encode(&ui.last_status)))
    });
    if let Some((label, text)) = update {
        unsafe {
            let _ = SetWindowTextW(label, PCWSTR(text.as_ptr()));
        }
    }
}

fn encode(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn monitor(commands: Receiver<Mode>, statuses: Sender<String>, chrome: bool) {
    let mut mode = Mode::Automatic;
    let mut probe = Probe::new(chrome);
    let mut signal = Signal::new();
    let mut display = String::new();
    loop {
        let status = match mode {
            Mode::Automatic => {
                let (evidence, scene) = match probe.sample() {
                    Ok(sample) => {
                        if display != sample.display {
                            display.clone_from(&sample.display);
                            signal = Signal::new();
                        }
                        (sample.evidence, describe(&sample))
                    }
                    Err(error) => (Evidence::Unknown, format!("无法采样显示器\r\n{error}")),
                };
                let desired = signal.correction(Instant::now(), evidence);
                let setting = desired.map_or_else(settings::current, settings::apply);
                format_status("自动", &scene, setting)
            }
            Mode::Manual(enabled) => {
                let setting = settings::apply(enabled);
                if statuses
                    .send(format_status("手动", "已暂停自动识别", setting))
                    .is_err()
                {
                    return;
                }
                // In manual mode, set the value once and let the user change it in Snipping Tool.
                loop {
                    match commands.recv_timeout(Duration::from_secs(1)) {
                        Ok(next) => {
                            mode = next;
                            break;
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if statuses
                                .send(format_status("手动", "已暂停自动识别", settings::current()))
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                continue;
            }
        };
        if statuses.send(status).is_err() {
            return;
        }
        match commands.recv_timeout(Duration::from_secs(1)) {
            Ok(next) => {
                mode = next;
                if matches!(mode, Mode::Manual(_)) {
                    probe = Probe::new(chrome);
                    signal = Signal::new();
                    display.clear();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn describe(sample: &Measurement) -> String {
    let label = match sample.evidence {
        Evidence::DisplaySdr => "显示器处于 SDR 模式",
        Evidence::Highlights => "检测到 HDR 高光",
        Evidence::HdrPlayback => "Chrome 正在显示哔哩哔哩 HDR10 画面",
        Evidence::WithinSdrRange => "画面亮度处于 SDR 范围",
        Evidence::Unknown => "暂时无法判断画面亮度",
    };
    let display = &sample.display;
    match sample.levels {
        Some(levels) => format!(
            "{display}  {label}\r\nSDR 白场 {:.0} nit  ·  采样峰值 {:.0} nit",
            levels.white_nits, levels.peak_nits
        ),
        None => format!("{display}  {label}"),
    }
}

fn format_status(mode: &str, scene: &str, setting: std::io::Result<bool>) -> String {
    let setting = match setting {
        Ok(true) => "开".into(),
        Ok(false) => "关".into(),
        Err(error) => format!("无法访问：{error}"),
    };
    format!("运行模式：{mode}\r\n{scene}\r\n\r\n截图工具 HDR 屏幕截图颜色更正器：{setting}")
}
