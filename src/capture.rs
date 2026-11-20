use half::f16;
use std::mem::size_of;
use windows::core::{Error, Interface, Result, HRESULT};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, E_FAIL, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, HMONITOR, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::signal::Evidence;

const SAMPLE_STRIDE: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct Levels {
    pub white_nits: f32,
    pub peak_nits: f32,
    pub highlight_count: usize,
    pub sample_count: usize,
}

impl Levels {
    fn evidence(self) -> Evidence {
        // Reject isolated overshoots while retaining small highlights.
        if self.highlight_count >= (self.sample_count / 2500).max(8) {
            Evidence::Highlights
        } else {
            Evidence::WithinSdrRange
        }
    }
}

pub struct Measurement {
    pub display: String,
    pub source: &'static str,
    pub evidence: Evidence,
    pub levels: Option<Levels>,
}

pub struct Probe {
    session: Option<Output>,
    chrome: Option<crate::browser::Chrome>,
}

impl Probe {
    pub fn new(chrome: bool) -> Self {
        Self {
            session: None,
            chrome: chrome.then(crate::browser::Chrome::new),
        }
    }

    pub fn sample(&mut self) -> Result<Measurement> {
        let result = self.sample_current_output();
        if result.is_err() {
            // Mode switches and desktop switches invalidate DXGI duplication sessions.
            self.session = None;
        }
        result
    }

    pub fn chrome_status(&self) -> Option<&str> {
        self.chrome.as_ref().map(crate::browser::Chrome::status)
    }

    fn sample_current_output(&mut self) -> Result<Measurement> {
        let mut pos = POINT::default();
        unsafe {
            GetCursorPos(&mut pos)?;
        }
        let monitor = unsafe { MonitorFromPoint(pos, MONITOR_DEFAULTTONEAREST) };
        let same_output = self.session.as_ref().is_some_and(|output| {
            output.monitor == monitor && unsafe { output.factory.IsCurrent().as_bool() }
        });
        if !same_output {
            // Release the old duplication before creating a replacement for the same output.
            self.session = None;
            self.session = Some(Output::open(monitor)?);
        }
        self.session
            .as_mut()
            .expect("DXGI output selected")
            .sample(self.chrome.as_mut())
    }
}

struct Output {
    monitor: HMONITOR,
    factory: IDXGIFactory1,
    adapter: IDXGIAdapter1,
    output: IDXGIOutput6,
    name: [u16; 32],
    capture: Option<Capture>,
}

impl Output {
    fn open(monitor: HMONITOR) -> Result<Self> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        for adapter_index in 0.. {
            let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                result => result?,
            };
            for output_index in 0.. {
                let output = match unsafe { adapter.EnumOutputs(output_index) } {
                    Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                    result => result?,
                };
                let desc = unsafe { output.GetDesc()? };
                if desc.AttachedToDesktop.as_bool() && desc.Monitor == monitor {
                    return Ok(Self {
                        monitor,
                        factory,
                        adapter,
                        output: output.cast()?,
                        name: desc.DeviceName,
                        capture: None,
                    });
                }
            }
        }
        Err(Error::new(E_FAIL, "找不到鼠标所在的 DXGI 显示器"))
    }

    fn sample(&mut self, chrome: Option<&mut crate::browser::Chrome>) -> Result<Measurement> {
        let desc = unsafe { self.output.GetDesc1()? };
        let display = String::from_utf16_lossy(wide_name(&self.name));
        if desc.ColorSpace != DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 {
            self.capture = None;
            return Ok(Measurement {
                display,
                source: "DXGI",
                evidence: Evidence::DisplaySdr,
                levels: None,
            });
        }
        if chrome.is_some_and(|browser| browser.hdr_is_visible(self.monitor)) {
            self.capture = None;
            return Ok(Measurement {
                display,
                source: "Chrome",
                evidence: Evidence::HdrPlayback,
                levels: None,
            });
        }
        let white_nits = sdr_white(&self.name)?;
        let capture = match &mut self.capture {
            Some(capture) => capture,
            empty => empty.insert(Capture::open(&self.adapter, &self.output, self.monitor)?),
        };
        let levels = capture.sample(white_nits)?;
        Ok(Measurement {
            display,
            source: capture.name(),
            evidence: levels.map_or(Evidence::Unknown, Levels::evidence),
            levels,
        })
    }
}

fn wide_name(data: &[u16]) -> &[u16] {
    &data[..data.iter().position(|&ch| ch == 0).unwrap_or(data.len())]
}

fn sdr_white(name: &[u16]) -> Result<f32> {
    let mut path_count = 0;
    let mut mode_count = 0;
    let status = unsafe {
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
    };
    if status != ERROR_SUCCESS {
        return Err(Error::from_hresult(status.to_hresult()));
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
    let status = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(Error::from_hresult(status.to_hresult()));
    }

    let mut found = None;
    for path in &paths[..path_count as usize] {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0
            || wide_name(&source.viewGdiDeviceName) != wide_name(name)
        {
            continue;
        }
        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };
        let status = unsafe { DisplayConfigGetDeviceInfo(&mut white.header) };
        if status != 0 {
            return Err(Error::from_hresult(HRESULT::from_win32(status as u32)));
        }
        if found.is_some_and(|previous| previous != white.SDRWhiteLevel) {
            return Err(Error::new(E_FAIL, "镜像显示器的 SDR 白场设置不一致"));
        }
        found = Some(white.SDRWhiteLevel);
    }
    match found {
        Some(level) if level > 0 => Ok(level as f32 * 80.0 / 1000.0),
        _ => Err(Error::new(E_FAIL, "无法读取该显示器的 SDR 白场")),
    }
}

enum Capture {
    Duplication(Duplicator),
    Graphics(crate::wgc::GraphicsCapture),
}

impl Capture {
    fn open(adapter: &IDXGIAdapter1, output: &IDXGIOutput6, monitor: HMONITOR) -> Result<Self> {
        let readback = Readback::open(adapter)?;
        let formats = [DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM];
        match unsafe { output.DuplicateOutput1(&readback.device, 0, &formats) } {
            Ok(duplication) => Ok(Self::Duplication(Duplicator {
                readback,
                duplication,
                last_levels: None,
            })),
            Err(error) if error.code() == DXGI_ERROR_UNSUPPORTED => Ok(Self::Graphics(
                crate::wgc::GraphicsCapture::open(monitor, readback)?,
            )),
            Err(error) => Err(Error::new(
                error.code(),
                format!("DXGI DuplicateOutput1: {error}"),
            )),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Duplication(_) => "DXGI",
            Self::Graphics(_) => "WGC",
        }
    }

    fn sample(&mut self, white_nits: f32) -> Result<Option<Levels>> {
        match self {
            Self::Duplication(capture) => capture.sample(white_nits),
            Self::Graphics(capture) => capture.sample(white_nits),
        }
    }
}

pub struct Readback {
    pub device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<ID3D11Texture2D>,
}

struct Duplicator {
    readback: Readback,
    duplication: IDXGIOutputDuplication,
    last_levels: Option<Levels>,
}

struct AcquiredFrame<'a>(&'a IDXGIOutputDuplication);

impl Drop for AcquiredFrame<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.ReleaseFrame();
        }
    }
}

impl Duplicator {
    fn sample(&mut self, white_nits: f32) -> Result<Option<Levels>> {
        let mut frame = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        let matching_levels = self
            .last_levels
            .filter(|levels| levels.white_nits == white_nits);
        match unsafe {
            self.duplication
                .AcquireNextFrame(100, &mut frame, &mut resource)
        } {
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(matching_levels),
            result => result?,
        }
        let _frame = AcquiredFrame(&self.duplication);
        if frame.ProtectedContentMaskedOut.as_bool() {
            self.last_levels = None;
        } else if frame.LastPresentTime != 0 || self.last_levels.is_none() {
            let texture: ID3D11Texture2D =
                resource.expect("DXGI returned the desktop image").cast()?;
            self.last_levels = self.readback.sample(&texture, white_nits)?;
        } else {
            return Ok(matching_levels);
        }
        Ok(self.last_levels)
    }
}

impl Readback {
    fn open(adapter: &IDXGIAdapter1) -> Result<Self> {
        let (mut device, mut context) = (None, None);
        unsafe {
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|error| Error::new(error.code(), format!("D3D11CreateDevice: {error}")))?;
        }
        let device = device.expect("D3D11CreateDevice returned a device");
        let context = context.expect("D3D11CreateDevice returned an immediate context");
        Ok(Self {
            device,
            context,
            staging: None,
        })
    }

    pub fn sample(&mut self, texture: &ID3D11Texture2D, white_nits: f32) -> Result<Option<Levels>> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }
        if desc.Format != DXGI_FORMAT_R16G16B16A16_FLOAT {
            return Ok(None);
        }

        let staging_matches = self.staging.as_ref().is_some_and(|staging| {
            let mut staging_desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                staging.GetDesc(&mut staging_desc);
            }
            staging_desc.Width == desc.Width && staging_desc.Height == desc.Height
        });
        if !staging_matches {
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            };
            unsafe {
                self.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut self.staging))?;
            }
        }
        let staging = self.staging.as_ref().expect("staging texture was created");
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let levels = unsafe {
            self.context.CopyResource(staging, texture);
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let levels = measure_pixels(
                &mapped,
                desc.Width as usize,
                desc.Height as usize,
                white_nits,
            );
            self.context.Unmap(staging, 0);
            levels
        };
        Ok(Some(levels))
    }
}

// The mapped DXGI surface contains four little-endian half-floats per pixel, with row padding.
unsafe fn measure_pixels(
    mapped: &D3D11_MAPPED_SUBRESOURCE,
    width: usize,
    height: usize,
    white_nits: f32,
) -> Levels {
    let threshold = white_nits + (0.10 * white_nits).max(12.0);
    let mut peak_nits: f32 = 0.0;
    let mut highlight_count = 0;
    let mut sample_count = 0;
    let pixels = mapped.pData.cast::<u8>();
    for y in (0..height).step_by(SAMPLE_STRIDE) {
        let row = pixels.add(y * mapped.RowPitch as usize).cast::<u16>();
        for x in (0..width).step_by(SAMPLE_STRIDE) {
            let rgb = row.add(x * 4);
            let red = f16::from_bits(rgb.read_unaligned()).to_f32();
            let green = f16::from_bits(rgb.add(1).read_unaligned()).to_f32();
            let blue = f16::from_bits(rgb.add(2).read_unaligned()).to_f32();
            let nits = 80.0 * (0.2126 * red + 0.7152 * green + 0.0722 * blue);
            if nits.is_finite() {
                peak_nits = peak_nits.max(nits);
                highlight_count += usize::from(nits > threshold);
                sample_count += 1;
            }
        }
    }
    Levels {
        white_nits,
        peak_nits,
        highlight_count,
        sample_count,
    }
}
