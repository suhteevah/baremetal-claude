//! WASI preview1 stubs for basic I/O.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::memory::LinearMemory;
use crate::types::Value;

/// WASI context holding state for WASI calls.
pub struct WasiCtx {
    pub args: Vec<Vec<u8>>,
    pub env_vars: Vec<(Vec<u8>, Vec<u8>)>,
    pub stdout_buf: Vec<u8>,
    pub exit_code: Option<i32>,
}

impl WasiCtx {
    pub fn new() -> Self {
        Self {
            args: Vec::new(),
            env_vars: Vec::new(),
            stdout_buf: Vec::new(),
            exit_code: None,
        }
    }
}

/// WASI errno constants.
const ERRNO_SUCCESS: i32 = 0;
const ERRNO_BADF: i32 = 8;
const ERRNO_NOSYS: i32 = 52;

/// fd_write: write data to a file descriptor.
/// params: [fd: i32, iovs_ptr: i32, iovs_len: i32, nwritten_ptr: i32]
/// returns: [errno: i32]
pub fn fd_write(
    args: &[Value],
    ctx: &mut WasiCtx,
    mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    let fd = args.get(0).and_then(|v| v.as_i32().ok()).unwrap_or(0);
    let iovs_ptr = args.get(1).and_then(|v| v.as_i32().ok()).unwrap_or(0) as u32;
    let iovs_len = args.get(2).and_then(|v| v.as_i32().ok()).unwrap_or(0) as u32;
    let nwritten_ptr = args.get(3).and_then(|v| v.as_i32().ok()).unwrap_or(0) as u32;

    if fd != 1 && fd != 2 {
        return Ok(vec![Value::I32(ERRNO_BADF)]);
    }

    let mem = match mem.as_ref() {
        Some(m) => m,
        None => return Ok(vec![Value::I32(ERRNO_BADF)]),
    };

    let mut total_written: u32 = 0;
    for i in 0..iovs_len {
        let iov_base_addr = iovs_ptr + i * 8;
        let buf_ptr = mem.read_u32_le(iov_base_addr)?;
        let buf_len = mem.read_u32_le(iov_base_addr + 4)?;

        let data = mem.read_bytes(buf_ptr, buf_len as usize)?;
        ctx.stdout_buf.extend_from_slice(data);
        total_written += buf_len;
    }

    // nwritten would need mutable memory access; handled by caller
    let _ = nwritten_ptr;
    let _ = total_written;

    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// fd_read: read from a file descriptor (stub — returns 0 bytes).
pub fn fd_read(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    // Stub: return 0 bytes read
    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// fd_close: close a file descriptor (stub).
pub fn fd_close(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// fd_seek: seek in a file descriptor (stub).
pub fn fd_seek(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_NOSYS)])
}

/// fd_prestat_get: get preopened fd info (stub).
pub fn fd_prestat_get(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_BADF)])
}

/// fd_prestat_dir_name: get preopened dir name (stub).
pub fn fd_prestat_dir_name(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_BADF)])
}

/// args_get: get command line arguments.
pub fn args_get(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// args_sizes_get: get argument count and total size.
/// params: [argc_ptr: i32, argv_buf_size_ptr: i32]
pub fn args_sizes_get(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    // Return 0 args, 0 size
    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// environ_get: get environment variables.
pub fn environ_get(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// environ_sizes_get: get env var count and total size.
pub fn environ_sizes_get(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// clock_time_get: get current time (stub — returns 0).
pub fn clock_time_get(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_SUCCESS)])
}

/// proc_exit: terminate the process.
pub fn proc_exit(
    args: &[Value],
    ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    let code = args.get(0).and_then(|v| v.as_i32().ok()).unwrap_or(0);
    ctx.exit_code = Some(code);
    Err(format!("proc_exit({})", code))
}

/// path_open: open a file path (stub).
pub fn path_open(
    _args: &[Value],
    _ctx: &mut WasiCtx,
    _mem: &mut Option<LinearMemory>,
) -> Result<Vec<Value>, String> {
    Ok(vec![Value::I32(ERRNO_NOSYS)])
}
