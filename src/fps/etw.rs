//! The ETW session behind [`super::Capture`] on Windows.

use anyhow::{bail, Context, Result};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, FILETIME};
use windows_sys::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, StartTraceW, CONTROLTRACE_HANDLE,
    ENABLE_TRACE_PARAMETERS, ENABLE_TRACE_PARAMETERS_VERSION_2, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_FILTER_DESCRIPTOR,
    EVENT_FILTER_EVENT_ID, EVENT_FILTER_TYPE_EVENT_ID, EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW,
    EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, PROCESSTRACE_HANDLE, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, TRACE_LEVEL_INFORMATION, WNODE_FLAG_TRACED_GUID,
};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;

const DXGKRNL: GUID = GUID::from_u128(0x802ec45a_1e99_4b83_9920_87c98277ba9d);
const PRESENT_INFO: u16 = 184;
const KEYWORD_PRESENT: u64 = 0x800_0000;
/// Presents held for a reader that stopped taking them: minutes of them at 240 fps.
const MAX_PENDING: usize = 100_000;

/// Now, in the unit and clock of ETW timestamps.
pub fn now() -> i64 {
    let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    unsafe { GetSystemTimePreciseAsFileTime(&mut ft) };
    (u64::from(ft.dwHighDateTime) << 32 | u64::from(ft.dwLowDateTime)) as i64
}

struct Shared {
    /// The process whose presents are kept, 0 for none.
    target: AtomicU32,
    presents: Mutex<Vec<i64>>,
}

/// The ETW session, named `session`. It stops when this is dropped.
pub struct Capture {
    shared: Arc<Shared>,
    control: CONTROLTRACE_HANDLE,
    trace: PROCESSTRACE_HANDLE,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start(session: &str) -> Result<Capture> {
        // A camless that was killed leaves its session running, and a second one by
        // that name cannot start.
        stop_by_name(session);
        let mut name = wide(session);
        let mut props = Properties::new();
        let mut control = CONTROLTRACE_HANDLE { Value: 0 };
        match unsafe { StartTraceW(&mut control, name.as_ptr(), &mut props.props) } {
            ERROR_SUCCESS => {}
            ERROR_ACCESS_DENIED => bail!("needs administrator rights"),
            ERROR_ALREADY_EXISTS => bail!("another ETW session is called {session}"),
            e => bail!("StartTrace failed ({e})"),
        }
        // Stops the session on the way out until the capture owns it.
        let started = Started(control);
        let ids = EVENT_FILTER_EVENT_ID { FilterIn: true, Reserved: 0, Count: 1, Events: [PRESENT_INFO] };
        let mut filter = EVENT_FILTER_DESCRIPTOR {
            Ptr: &ids as *const _ as u64,
            Size: size_of::<EVENT_FILTER_EVENT_ID>() as u32,
            Type: EVENT_FILTER_TYPE_EVENT_ID,
        };
        let params = ENABLE_TRACE_PARAMETERS {
            Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
            EnableProperty: 0,
            ControlFlags: 0,
            SourceId: GUID::default(),
            EnableFilterDesc: &mut filter,
            FilterDescCount: 1,
        };
        let e = unsafe {
            EnableTraceEx2(
                control,
                &DXGKRNL,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                TRACE_LEVEL_INFORMATION as u8,
                KEYWORD_PRESENT,
                0,
                0,
                &params,
            )
        };
        if e != ERROR_SUCCESS {
            bail!("EnableTraceEx2 failed ({e})");
        }
        let shared = Arc::new(Shared { target: AtomicU32::new(0), presents: Mutex::new(Vec::new()) });
        let mut log: Box<EVENT_TRACE_LOGFILEW> = Box::new(unsafe { std::mem::zeroed() });
        log.LoggerName = name.as_mut_ptr();
        log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
        log.Anonymous2.EventRecordCallback = Some(on_event);
        log.Context = Arc::as_ptr(&shared) as *mut c_void;
        let trace = unsafe { OpenTraceW(&mut *log) };
        if trace.Value == u64::MAX {
            bail!("OpenTrace failed: {}", std::io::Error::last_os_error());
        }
        // The callback reads `shared` through a raw pointer, so the thread keeps it, and
        // what OpenTrace was given, for as long as the trace runs.
        let keep = (shared.clone(), Opened { _log: log, _name: name });
        let thread = std::thread::Builder::new()
            .name("fps".into())
            .spawn(move || {
                let _keep = keep;
                unsafe { ProcessTrace(&trace, 1, std::ptr::null(), std::ptr::null()) };
            })
            .context("starting the fps thread");
        let thread = match thread {
            Ok(t) => t,
            Err(e) => {
                unsafe { CloseTrace(trace) };
                return Err(e);
            }
        };
        std::mem::forget(started);
        Ok(Capture { shared, control, trace, thread: Some(thread) })
    }

    /// Keeps the presents of process `pid` from now on, 0 for none. Changing it drops
    /// the presents not taken yet.
    pub fn follow(&self, pid: u32) {
        if self.shared.target.swap(pid, Ordering::Relaxed) != pid {
            self.shared.presents.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    /// The presents since the last call, in no particular order.
    pub fn take(&self) -> Vec<i64> {
        std::mem::take(&mut *self.shared.presents.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// False once the session ended without us, say stopped with `logman`.
    pub fn alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for Capture {
    /// Does not wait for the thread: this runs on the thread that drives the fans. The
    /// thread ends by itself once the trace is closed.
    fn drop(&mut self) {
        stop(self.control);
        unsafe { CloseTrace(self.trace) };
    }
}

struct Opened {
    _log: Box<EVENT_TRACE_LOGFILEW>,
    _name: Vec<u16>,
}

// Only the fps thread holds it, and nothing reads it.
unsafe impl Send for Opened {}

struct Started(CONTROLTRACE_HANDLE);

impl Drop for Started {
    fn drop(&mut self) {
        stop(self.0);
    }
}

/// Runs on the fps thread for each event, so it must not panic.
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    let r = &*record;
    let shared = &*(r.UserContext as *const Shared);
    let h = &r.EventHeader;
    if h.EventDescriptor.Id != PRESENT_INFO || h.ProcessId == 0 || h.ProcessId != shared.target.load(Ordering::Relaxed) {
        return;
    }
    let mut presents = shared.presents.lock().unwrap_or_else(|e| e.into_inner());
    if presents.len() < MAX_PENDING {
        presents.push(h.TimeStamp);
    }
}

/// EVENT_TRACE_PROPERTIES with room after it for the session name, which Windows
/// writes there.
#[repr(C)]
struct Properties {
    props: EVENT_TRACE_PROPERTIES,
    name: [u16; 64],
}

impl Properties {
    fn new() -> Box<Properties> {
        let mut p: Box<Properties> = Box::new(unsafe { std::mem::zeroed() });
        p.props.Wnode.BufferSize = size_of::<Properties>() as u32;
        p.props.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        // Timestamps from the performance counter, the finest clock.
        p.props.Wnode.ClientContext = 1;
        p.props.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        // Buffers go out at least once a second; SETTLE leaves time for that.
        p.props.FlushTimer = 1;
        p.props.BufferSize = 64;
        p.props.MinimumBuffers = 4;
        p.props.MaximumBuffers = 16;
        p.props.LoggerNameOffset = std::mem::offset_of!(Properties, name) as u32;
        p
    }
}

fn stop(control: CONTROLTRACE_HANDLE) {
    let mut props = Properties::new();
    unsafe { ControlTraceW(control, std::ptr::null(), &mut props.props, EVENT_TRACE_CONTROL_STOP) };
}

fn stop_by_name(session: &str) {
    let name = wide(session);
    let mut props = Properties::new();
    unsafe { ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, name.as_ptr(), &mut props.props, EVENT_TRACE_CONTROL_STOP) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
