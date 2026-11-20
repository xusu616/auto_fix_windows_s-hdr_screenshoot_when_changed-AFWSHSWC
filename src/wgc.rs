use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;
use windows::core::{factory, Error, Interface, Result};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
    IGraphicsCaptureSession5,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

use crate::capture::{Levels, Readback};

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::R16G16B16A16Float;
const BUFFERS: i32 = 2;

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

pub struct GraphicsCapture {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    device: IDirect3DDevice,
    readback: Readback,
    arrivals: Receiver<()>,
    size: SizeInt32,
    last_levels: Option<Levels>,
    _apartment: Apartment,
}

impl GraphicsCapture {
    pub fn open(monitor: HMONITOR, readback: Readback) -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        let apartment = Apartment;
        if !GraphicsCaptureSession::IsSupported()? {
            return Err(Error::new(
                E_FAIL,
                "当前会话不支持 Windows Graphics Capture",
            ));
        }
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor)? };
        let dxgi: IDXGIDevice = readback.device.cast()?;
        let device: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&device, FORMAT, BUFFERS, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        let (ready, arrivals) = mpsc::sync_channel(1);
        let state = Self {
            session,
            pool,
            device,
            readback,
            arrivals,
            size,
            last_levels: None,
            _apartment: apartment,
        };
        state
            .pool
            .FrameArrived(&TypedEventHandler::new(move |_, _| {
                let _ = ready.try_send(());
                Ok(())
            }))?;
        state.session.SetIsCursorCaptureEnabled(false)?;
        // Windows still displays the capture indicator when required by its consent settings.
        state.session.SetIsBorderRequired(false)?;
        if state.session.cast::<IGraphicsCaptureSession5>().is_ok() {
            state.session.SetMinUpdateInterval(TimeSpan {
                Duration: 5_000_000,
            })?;
        }
        state.session.StartCapture()?;
        Ok(state)
    }

    pub fn sample(&mut self, white_nits: f32) -> Result<Option<Levels>> {
        match self.arrivals.recv_timeout(Duration::from_millis(180)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => {
                return Ok(self
                    .last_levels
                    .filter(|levels| levels.white_nits == white_nits));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::new(E_FAIL, "Windows Graphics Capture 帧事件已断开"));
            }
        }
        let frame = self.pool.TryGetNextFrame()?;
        let size = frame.ContentSize()?;
        if size != self.size {
            frame.Close()?;
            self.pool.Recreate(&self.device, FORMAT, BUFFERS, size)?;
            self.size = size;
            self.last_levels = None;
        } else {
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
            self.last_levels = self.readback.sample(&texture, white_nits)?;
            frame.Close()?;
        }
        Ok(self.last_levels)
    }
}

impl Drop for GraphicsCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
