use std::io::{self, Error, ErrorKind};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, FILETIME, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegLoadAppKeyW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, KEY_READ,
    KEY_SET_VALUE, REG_VALUE_TYPE,
};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;

const HIVE_PATH: &str = r"Packages\Microsoft.ScreenSketch_8wekyb3d8bbwe\Settings\settings.dat";
const BOOLEAN_TYPE: REG_VALUE_TYPE = REG_VALUE_TYPE(0x05F5E10B);
const TONE_MAPPING: PCWSTR = w!("IsHDRToneMappingEnabled");

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(Error::from_raw_os_error(status.0 as i32))
    }
}

fn open_local_state(write: bool) -> io::Result<Key> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "无法定位当前用户的 LocalAppData"))?;
    let path = PathBuf::from(local).join(HIVE_PATH);
    if !path.try_exists()? {
        return Err(Error::new(ErrorKind::NotFound, "截图工具尚未创建设置文件"));
    }
    let mut path_utf16: Vec<u16> = path.as_os_str().encode_wide().collect();
    path_utf16.push(0);
    let access = if write {
        KEY_READ | KEY_SET_VALUE
    } else {
        KEY_READ
    };
    let mut root = Key(HKEY::default());
    let mut local_state = Key(HKEY::default());
    unsafe {
        check(RegLoadAppKeyW(
            PCWSTR(path_utf16.as_ptr()),
            &mut root.0,
            access.0,
            0,
            0,
        ))?;
        check(RegOpenKeyExW(
            root.0,
            w!("LocalState"),
            0,
            access,
            &mut local_state.0,
        ))?;
    }
    Ok(local_state)
}

fn read(key: &Key) -> io::Result<bool> {
    let mut kind = REG_VALUE_TYPE::default();
    let mut data = [0u8; 9];
    let mut size = data.len() as u32;
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            TONE_MAPPING,
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut size),
        )
    };
    check(result)?;
    if kind != BOOLEAN_TYPE || size != data.len() as u32 || data[0] > 1 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "截图工具 HDR 设置格式与已验证版本不同",
        ));
    }
    Ok(data[0] == 1)
}

pub fn current() -> io::Result<bool> {
    read(&open_local_state(false)?)
}

pub fn apply(enabled: bool) -> io::Result<bool> {
    let key = open_local_state(true)?;
    if read(&key)? == enabled {
        return Ok(enabled);
    }
    let FILETIME {
        dwLowDateTime,
        dwHighDateTime,
    } = unsafe { GetSystemTimePreciseAsFileTime() };
    // Windows.ApplicationData stores the Boolean followed by its FILETIME timestamp.
    let mut data = [0u8; 9];
    data[0] = u8::from(enabled);
    data[1..5].copy_from_slice(&dwLowDateTime.to_le_bytes());
    data[5..9].copy_from_slice(&dwHighDateTime.to_le_bytes());
    unsafe {
        check(RegSetValueExW(
            key.0,
            TONE_MAPPING,
            0,
            BOOLEAN_TYPE,
            Some(&data),
        ))?;
    }
    read(&key)
}
