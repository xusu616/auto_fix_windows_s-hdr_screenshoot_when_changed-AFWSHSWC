#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod browser;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod settings;
#[cfg(any(windows, test))]
mod signal;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod wgc;

#[cfg(windows)]
fn main() {
    let mut args = std::env::args().skip(1);
    let mut action = args.next();
    let chrome = action.as_deref() == Some("--chrome-hdr");
    if chrome {
        action = args.next();
    }
    match action.as_deref() {
        Some("--probe") => {
            attach_console();
            let count = match args.next() {
                None => 10,
                Some(value) => match value.parse::<u32>() {
                    Ok(count) if count > 0 => count,
                    _ => {
                        eprintln!("Usage: AFWSHSWC --probe [positive sample count]");
                        std::process::exit(2);
                    }
                },
            };
            diagnostic(count, chrome);
            return;
        }
        Some("--setting") => {
            attach_console();
            match settings::current() {
                Ok(enabled) => println!("IsHDRToneMappingEnabled={}", u8::from(enabled)),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
            return;
        }
        Some(argument) => {
            attach_console();
            eprintln!("Unknown argument: {argument}");
            std::process::exit(2);
        }
        None => {}
    }
    if let Err(error) = tray::run(chrome) {
        tray::show_error(&error.to_string());
    }
}

#[cfg(windows)]
fn attach_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(windows)]
fn diagnostic(count: u32, chrome: bool) {
    use std::time::Duration;
    let mut probe = capture::Probe::new(chrome);
    for index in 0..count {
        if index > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        match probe.sample() {
            Ok(result) => {
                print!("{} {} {:?}", result.display, result.source, result.evidence);
                if let Some(levels) = result.levels {
                    print!(
                        " | SDR white {:.1} nits | peak {:.1} nits | highlights {}/{}",
                        levels.white_nits,
                        levels.peak_nits,
                        levels.highlight_count,
                        levels.sample_count
                    );
                }
                if chrome && result.evidence != signal::Evidence::DisplaySdr {
                    print!(" | Chrome: {}", probe.chrome_status().unwrap());
                }
                println!();
            }
            Err(error) => println!("Unknown: {error}"),
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("AFWSHSWC runs on Windows.");
}
